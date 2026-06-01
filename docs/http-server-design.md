# HTTP Server — Component Design

## Overview

The HTTP service exposes MemoryHub over a small JSON API consumed by agents. It is a thin Axum forwarder: each route deserializes a JSON body, sends one message to the `MemoryManager` actor, and serializes the reply. It also owns one piece of state — an `AuthStore` — and runs authentication middleware before every handler.

A single `HttpServer` actor (child of `MemoryHub`) owns the `axum::serve` task and aborts it on stop. It has no message handlers; the handlers are plain async functions holding the `MemoryManager` address and the `AuthStore` as Axum state. The actor wrapper exists only for supervision and shared shutdown — the request path never goes through its mailbox.

## Authentication

Every `/v1` route except `/v1/health` requires `Authorization: Bearer <secret>`, and the token is the **sole source of identity** — no body/header override, no impersonation.

Middleware (`from_fn_with_state`) resolves the secret to a `Principal`:

1. Missing/invalid header → **401**.
2. Matches the root admin token (compared as SHA-256 digests to avoid length/early-exit leaks) **and no admin user exists** → `Principal::Root`. Root is bootstrap-only (see Config & Bootstrap).
3. Otherwise `AuthStore::resolve_token` (SHA-256 lookup in `spawn_blocking`); a hit → `Principal::User { username, role }`, a miss/expired → **401**.

The `Principal` is inserted into request extensions; two extractors read it:

- `AuthUser` — requires a real user, yields `username`; rejects `Root` (it has no memory namespace).
- `AdminPrincipal` — requires `Root` or `role == "admin"`, else **403**.

`AuthUser.username` is the namespace for every **write/read/delete**: the handler fills `username` from the token, not the request body, so a user only mutates and reads files under their own `{username}/…` tree. **Search is the exception** — its `scope` may be `all` (the default), which spans every user's memories and the shared summaries, so search deliberately reaches beyond the caller's namespace. Internal actor messages still carry `username` from the token.

## AuthStore

A plain struct over `Arc<Mutex<Connection>>` on its own `auth.db`, opened in `HttpServer::post_start`. It is **not** an actor (SQLite serializes access and it shares the server's lifecycle) and runs blocking queries in `spawn_blocking`. It uses a separate `auth.db` rather than `memoryhub.db` so token lookups never contend with the Indexer's write mutex.

It stores two tables: `users` (`username` primary key, `role`) and `tokens` (uuid `id`, owning `username`, the SHA-256 hex of the secret, optional `name` and `expires_at`). Foreign keys are on, so deleting a user cascades to their tokens.

A token secret is `mh_` + base64url of 32 random bytes, returned **once** at creation; only its SHA-256 hex is stored (high entropy, so no slow password hash needed; the `mh_` prefix aids leak scanning). Revocation deletes the row. `expires_at` is an optional unix-seconds timestamp; a token past it resolves as invalid.

## Endpoints

All bodies are JSON; all routes nest under `/v1`. `filename` is opaque — any `/` or `\` is flattened to `_` server-side. `project` is an optional single segment (omitted → the reserved `_default` bucket; a value containing `/`/`\` or equal to `_synthesized`/`_default` is rejected with 400). Memory/search bodies carry no `username` (it comes from the token); token secrets are returned only by the mint endpoint.

| Visibility | Method & path                            | Body                                      | Reply                                           |
| ---------- | ---------------------------------------- | ----------------------------------------- | ----------------------------------------------- |
| Public     | `GET /v1/health`                         | —                                         | `{"status":"ok"}`                               |
| User       | `POST /v1/memories/write`                | `{agent_id, project?, filename, content}` | `{}`                                            |
| User       | `POST /v1/memories/read`                 | `{agent_id, filename}`                    | `{"content":…}` or 404                          |
| User       | `POST /v1/memories/delete`               | `{agent_id, filename}`                    | `{}`                                            |
| User       | `POST /v1/memories/search`               | `{agent_id?, scope?, raw_only?, query}`   | `{"results":[…]}`                               |
| User       | `GET /v1/me`                             | —                                         | `{username, role}`                              |
| Admin      | `POST /v1/admin/users`                   | `{username, role}`                        | create user                                     |
| Admin      | `GET /v1/admin/users`                    | —                                         | `{users:[{username, role, created_at}]}`        |
| Admin      | `DELETE /v1/admin/users/{username}`      | —                                         | cascades tokens                                 |
| Admin      | `POST /v1/admin/users/{username}/tokens` | `{name?, expires_at?}`                    | `{id, token}` (secret returned once)            |
| Admin      | `GET /v1/admin/users/{username}/tokens`  | —                                         | `{tokens:[{id, name, created_at, expires_at}]}` |
| Admin      | `DELETE /v1/admin/tokens/{id}`           | —                                         | revoke                                          |

## Config & Bootstrap

```toml
[auth]
db_path     = "auth.db"   # resolved against base_dir
admin_token = "..."       # overridden by MEMORYHUB_ADMIN_TOKEN (preferred)
```

The root admin token is optional and **bootstrap-only**: it provisions the first admin on a fresh deploy and is ignored once any admin user exists (reviving automatically if every admin is later removed). This keeps the env-stored secret useful only during bootstrap. If neither a root token nor an admin user exists, management is unreachable and the server logs a startup warning.

Bootstrap: start with a root token → `POST /v1/admin/users` with `role:"admin"` → mint that user a token → manage with the admin token thereafter.

## Error Handling

`HttpError` implements `IntoResponse`; non-2xx bodies share the shape `{"error": <code>, "message"?: <string>}`.

| Source                                             | Status | Code           | `message`?            |
| -------------------------------------------------- | ------ | -------------- | --------------------- |
| JSON deserialization failure (Axum)                | 400    | —              | Axum's default body   |
| Invalid `project` (separator or reserved name)     | 400    | `bad_request`  | yes                   |
| Missing / invalid / expired token                  | 401    | `unauthorized` | no                    |
| Authenticated but not admin                        | 403    | `forbidden`    | no                    |
| Missing file, `UserNotFound`, `TokenNotFound`      | 404    | `not_found`    | no                    |
| `UserExists`                                       | 409    | `conflict`     | no                    |
| `MemoryError` / `AuthError::Db` from a store/actor | 500    | `internal`     | yes (error `Display`) |
| Mailbox/send failure (actor dead)                  | 503    | `unavailable`  | no                    |

The JSON-deserialization 400 uses Axum's built-in `Json` rejection body; the validation 400 (invalid `project`) uses the unified shape. The 500 `message` is the error's `Display`, user-safe by codebase convention. A `tower_http` `TraceLayer` emits a span per request; no retry/rate-limit in this iteration.

## Out of Scope

`last_used_at`, token rotation, granular scopes; memory list/enumerate endpoints; rate limiting, sessions, OAuth, password auth.
