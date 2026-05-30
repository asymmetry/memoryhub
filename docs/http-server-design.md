# HTTP Server — Component Design

## Overview

The HTTP service exposes MemoryHub over a small JSON API consumed by agents. It
is a thin Axum-based forwarder: each route deserializes a JSON body, sends one
message to the `MemoryManager` actor, and serializes the reply. Beyond
forwarding it owns one piece of state — an `AuthStore` (user + token storage) —
and one cross-cutting concern: authentication middleware that runs before every
handler.

A single `HttpServer` actor (child of `MemoryHub`, sibling to `MemoryManager`
and `LlmService`) owns the `axum::serve` task and aborts it on stop. It has no
message handlers — the handlers are plain async functions holding the
`MemoryManager` address and the `AuthStore` as Axum state. Keeping the server
inside an actor is purely so it participates in supervision and shared shutdown;
the request path itself never goes through the actor's mailbox.

## Authentication

Every `/v1` route except `/v1/health` requires a bearer token, and the
authenticated user is the **sole source of identity** — there is no body/header
override and no impersonation. Auth is an HTTP-edge access-control concern, so
it lives in this layer rather than as a peer domain actor of `MemoryManager`.

### Principals & middleware

An Axum `from_fn_with_state` middleware (with `HttpServerState`, so it can reach
`AuthStore`) runs on all `/v1` routes except `/v1/health`:

1. Read `Authorization: Bearer <secret>`; absent → **401**.
2. If `<secret>` matches the configured root admin token (compared as SHA-256
   digests to avoid length/early-exit leaks) → `Principal::Root`.
3. Else call `AuthStore::resolve_token(secret)` (SHA-256 lookup, in
   `spawn_blocking`); a hit yields `Principal::User { username, role }`, a
   miss → **401**.

The resolved `Principal` is inserted into request extensions. Two extractors
read it:

- `AuthUser` — requires a real user; yields `username`. `Principal::Root` is
  rejected (the root token has no memory namespace).
- `AdminPrincipal` — requires `Root` or `role == "admin"`; otherwise **403**.

A `Principal::User`'s `username` scopes every memory and search operation to
that user's namespace. The internal actor messages (`FileOpWrite`, `Search`, …)
still carry `username`; the handler fills it from `AuthUser` rather than from the
request body. This is the only change to the memory subsystem.

### AuthStore

`AuthStore` is a plain struct wrapping `Arc<Mutex<Connection>>` over its own
`auth.db`, opened in `HttpServer::post_start` and exposed through
`HttpServerState`. Like the Indexer it runs blocking queries in `spawn_blocking`,
but it is **not** an actor — SQLite's own locking serializes access,
supervision/restart would only reopen a connection, and it shares the
`HttpServer`'s lifecycle (auth is useless without the HTTP API). It uses a
separate `auth.db` rather than `memoryhub.db` so per-request token lookups never
contend with the Indexer's embedding-write mutex.

### Tokens

A token secret is `mh_` followed by base64url of 32 random bytes (`rand`),
returned **once** at creation and never stored; only its SHA-256 (hex, via
`sha2`) is persisted. Random high-entropy secrets need no slow password hash, so
SHA-256 suffices; the `mh_` prefix aids leak scanning. Revocation deletes the
row — there is no `revoked` flag. `expires_at` is a unix timestamp in seconds; a
token past its `expires_at` resolves as invalid (not auto-deleted in this
iteration).

## Data Model (`auth.db`)

```sql
CREATE TABLE users (
    username   TEXT PRIMARY KEY,
    role       TEXT NOT NULL DEFAULT 'user',   -- 'user' | 'admin'
    created_at INTEGER NOT NULL
);

CREATE TABLE tokens (
    id         TEXT PRIMARY KEY,               -- uuid; safe to expose, used for revoke
    username   TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,           -- sha256 hex of the secret
    name       TEXT,                           -- optional label
    created_at INTEGER NOT NULL,
    expires_at INTEGER                         -- nullable; NULL = never expires
);
```

Foreign keys are enforced (`PRAGMA foreign_keys = ON`), so deleting a user
cascades to their tokens.

## Endpoints

All request and response bodies are JSON; all routes are nested under `/v1`.
`filename` is opaque: any `/` or `\` is flattened to `_` server-side, so the
agent encodes any logical sub-path into the name. A read of a missing file maps
`Ok(None)` to 404.

| Visibility | Method & path | Body | Actor / reply |
| ---------- | ------------- | ---- | ------------- |
| Public | `GET /v1/health` | — | `{"status":"ok"}` |
| User | `POST /v1/memories/write` | `{agent_id, filename, content}` | `FileOpWrite` → `{}` |
| User | `POST /v1/memories/read` | `{agent_id, filename}` | `FileOpRead` → `{"content":…}` or 404 |
| User | `POST /v1/memories/delete` | `{agent_id, filename}` | `FileOpDelete` → `{}` |
| User | `POST /v1/search` | `{agent_id, query}` | `Search` → `{"results":[…]}` |
| User | `GET /v1/me` | — | `{username, role}` |
| Admin | `POST /v1/admin/users` | `{username, role}` | create user |
| Admin | `GET /v1/admin/users` | — | `{users:[{username, role, created_at}]}` |
| Admin | `DELETE /v1/admin/users/{username}` | — | cascades tokens |
| Admin | `POST /v1/admin/users/{username}/tokens` | `{name?, expires_at?}` | returns `{id, token}` once |
| Admin | `GET /v1/admin/users/{username}/tokens` | — | `{tokens:[{id, name, created_at, expires_at}]}` |
| Admin | `DELETE /v1/admin/tokens/{id}` | — | revoke |

The memory/search request bodies no longer carry `username` (it comes from the
token). Token secrets are returned only by the mint endpoint and never listed.

## Config & Bootstrap

New `[auth]` section in `config.toml`:

```toml
[auth]
db_path     = "auth.db"   # resolved against base_dir like other data paths
admin_token = "..."       # overridden by MEMORYHUB_ADMIN_TOKEN env (preferred for Docker)
```

The root admin token is read from `MEMORYHUB_ADMIN_TOKEN` (preferred) or
`[auth].admin_token`. It is optional: admin routes always honor an `admin`-role
user's token; the root token only adds a break-glass credential and is the only
way to bootstrap a fresh deploy. If no root token is configured **and** no admin
user exists, management is unreachable, so the server logs a startup warning;
admin routes then simply have no caller that can satisfy `AdminPrincipal`.
Existing user tokens in `auth.db` keep working regardless.

Bootstrap: start with a root token → `POST /v1/admin/users` with `role:"admin"`
→ mint that user a token → use the root token thereafter only as break-glass.

## AuthStore API

`AuthStore` is a plain struct over `Arc<Mutex<Connection>>`; each method runs its
DB work in `spawn_blocking` and returns a `Future`. No actor messages or
`Handler` impls are involved.

| Method | Result |
| ------ | ------ |
| `resolve_token(secret)` | `Option<Principal>` |
| `create_user(username, role)` | `Result<UserInfo, AuthError>` |
| `list_users()` | `Result<Vec<UserInfo>, AuthError>` |
| `delete_user(username)` | `Result<(), AuthError>` |
| `create_token(username, name, expires_at)` | `Result<NewToken, AuthError>` (`{id, secret}`) |
| `list_tokens(username)` | `Result<Vec<TokenInfo>, AuthError>` |
| `revoke_token(id)` | `Result<(), AuthError>` |

Errors are surfaced to the calling handler and logged with `e.report()`.

## Error Handling

`HttpError` is one enum implementing `IntoResponse`; non-2xx bodies share the
shape `{"error": <code>, "message"?: <string>}`. `AuthError` (UserExists,
UserNotFound, TokenNotFound, Db) maps into it.

| Source | Status | Code | `message`? |
| ------ | ------ | ---- | ---------- |
| JSON deserialization failure (Axum) | 400 | — | Axum's default rejection body |
| Missing / invalid / expired token | 401 | `unauthorized` | no |
| Authenticated but not admin | 403 | `forbidden` | no |
| `FileOpRead` returns `Ok(None)`, `UserNotFound`, `TokenNotFound` | 404 | `not_found` | no |
| `UserExists` | 409 | `conflict` | no |
| `MemoryError` / `AuthError::Db` from an actor or store call | 500 | `internal` | yes (error `Display`) |
| Mailbox/send failure (actor dead) | 503 | `unavailable` | no |

The 500 `message` is the error's `Display` form, which is user-safe by codebase
convention — not a debug dump. 400 comes from Axum's built-in `Json` extractor
rejection and so does not use the unified shape. A `tower_http` `TraceLayer`
emits a span per request; there is no retry or rate-limit middleware in this
iteration.

## Testing

- **Unit** (`AuthStore`): create/list/delete users, mint/list/revoke tokens,
  hashing + secret format, expiry resolution, cascade on user delete. In-memory
  `auth.db`, methods called directly, no network.
- **Middleware/extractors**: 401 (missing/bad/expired token), 403 (user vs
  admin), root-token acceptance, `AuthUser` rejects `Root`.
- **Router**: existing memory/search tests mint a real user token in-process and
  send `Authorization`; bodies no longer include `username`.
- **Integration** (`test_integration.sh`): set a root token, create a user +
  token over the admin API, then exercise write/read/search with that token.

## Migration Impact

`username` leaving the request bodies and the new bearer requirement are a
breaking change to clients. The Claude Code plugin
(`agents/claude-code/memoryhub.py`) must send `Authorization: Bearer <token>`
and drop `username`; `/mh-config` gains a token prompt. This rides along with
the auth work.

## Out of Scope (Future Work)

- Cross-user / shared-namespace read and search policy (each token stays scoped
  to its own user) — tied to the future MCP / sharing work.
- `last_used_at` tracking, token rotation, granular scopes/permissions.
- Memory list/enumerate endpoints, readiness probe distinct from `/v1/health`.
- Streaming responses, rate limiting, login sessions, OAuth, password auth.
