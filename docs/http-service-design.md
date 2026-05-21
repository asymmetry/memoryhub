# HTTP Service — Component Design

## Overview

The HTTP service exposes MemoryHub over a small JSON API consumed by OpenClaw agents. It is a thin Axum-based forwarder: each route deserializes a JSON body, sends one message to the `MemoryManager` actor, and serializes the reply. It owns no business logic and no state beyond the `MemoryManager` address.

## Architecture

A single actor, `HttpServer`, is a child of the `Manager` supervisor (sibling to `MemoryManager` and `LlmService`).

```
                 Manager
              ______|______
             |      |      |
        HttpServer  MemoryManager   LlmService
             |
       axum::serve task
             |
        Router (handlers)
             |
        MemoryManager addr
```

On startup the actor:

1. Receives `ServerConfig` (host + port) and the `MemoryManager` address at construction.
2. Builds an `axum::Router` whose state is the cloned `MemoryManager` address.
3. Binds a `TcpListener` and spawns `axum::serve(...)` as a Tokio task, keeping the `JoinHandle`.

On stop the actor aborts the serve task. The actor receives no external messages — its only role is owning the server task and integrating it into supervision.

Handlers are plain async functions with `State<Addr<MemoryManager>>`, not actor handlers. Each handler:

1. Deserializes the JSON request body (Axum's `Json<T>` extractor).
2. Builds the corresponding actor message.
3. `addr.send(msg).await` to the `MemoryManager`.
4. Maps the reply to either `Json<T>` or an `HttpError`.

## Endpoints

All bodies are JSON. All response bodies are JSON. All routes are nested under the `/v1` prefix.

| Method | Path                  | Body                                                   | Actor message  | 200 reply                          |
| ------ | --------------------- | ------------------------------------------------------ | -------------- | ---------------------------------- |
| GET    | `/v1/health`          | —                                                      | —              | `{"status":"ok"}`                  |
| POST   | `/v1/memories/write`  | `{username, agent_id, filename, content}`              | `FileOpWrite`  | `{}`                               |
| POST   | `/v1/memories/read`   | `{username, agent_id, filename}`                       | `FileOpRead`   | `{"content": "..."}` or 404        |
| POST   | `/v1/memories/delete` | `{username, agent_id, filename}`                       | `FileOpDelete` | `{}`                               |
| POST   | `/v1/search`          | `{username, agent_id, query}`                          | `Search`       | `{"results": [SearchResult, ...]}` |

### Field conventions

- `username` — string
- `agent_id` — UUID string (e.g. `"550e8400-e29b-41d4-a716-446655440000"`)
- `filename` — string (e.g. `"2026-05-13.md"` or `"notes/2026-05-13.md"`). Opaque to the service; any `/` or `\` is flattened to `_` server-side. The agent is responsible for encoding any logical sub-path into this name.

`SearchResult` is serialized as-is from `crate::memory::messages::SearchResult` (`path`, `start_line`, `end_line`, `score`, `snippet`).

### Identity

Identity (`username`, `agent_id`) is carried in the JSON request body, mirroring the actor message shape. There is no authentication, no header inspection, no path-embedded identity in this iteration.

### Read semantics

Read maps `Ok(None)` from `FileOpRead` to a 404. `Ok(Some(content))` maps to 200 with `{"content": content}`.

## Error Handling

Handlers return `Result<Json<T>, HttpError>`. `HttpError` is a single enum implementing `axum::response::IntoResponse`. All non-2xx responses share the shape `{"error": <code>, "message"?: <string>}`.

| Source                                       | Status | Body                                         |
| -------------------------------------------- | ------ | -------------------------------------------- |
| JSON deserialization failure (Axum built-in) | 400    | `{"error": "bad_request", "message": "..."}` |
| `FileOpRead` returns `Ok(None)`              | 404    | `{"error": "not_found"}`                     |
| `MemoryError` from any actor call            | 500    | `{"error": "internal", "message": "..."}`    |
| Mailbox/send failure (actor dead)            | 503    | `{"error": "unavailable"}`                   |

The `message` field is included for 400 and 500 only. The 500 `message` is the `Display` form of the underlying error — already user-safe by codebase convention — not a debug dump. 404 and 503 are self-describing and omit `message`.

Handlers do the mapping with a small `map_err` per call; there are no `From<MemoryError>` or `From<SendError>` impls on `HttpError` because the mapping is trivial and explicit.

A `tower_http::trace::TraceLayer` is attached at INFO level so every request emits a structured `tracing` span. No middleware for retries, rate limiting, or auth in this iteration.

## Module Layout

Follows the project convention (`foo.rs` + `foo/`, no `mod.rs`):

```
src/
  http.rs              // HttpServer actor, spawn logic, re-exports
  http/
    error.rs           // HttpError enum + IntoResponse impl
    handlers.rs        // free async functions: write, read, delete, search, health
    router.rs          // build_router(addr) -> axum::Router
    dto.rs             // request/response JSON structs with serde derives
```

The actor in `http.rs` is small — it owns the `JoinHandle` of `axum::serve` and aborts it on stop. It has no message handlers.

## Wiring

The supervisor (`Manager`) spawns `LlmService` and `MemoryManager` first, then constructs `HttpServer` with `(server_config, memory_manager_addr)` and spawns it. `ServerConfig` is already defined in `src/config.rs` (host + port, defaults `0.0.0.0:8080`).

## Testing

- **Unit tests on `dto.rs`** — serde round-trip for each request/response struct.
- **Unit tests on `error.rs`** — each `HttpError` variant maps to the documented status + body.
- **Integration tests under `tests/`** — build the router with a stub `MemoryManager` actor (acktor test harness), drive requests via `tower::ServiceExt::oneshot`, assert status and body for the happy path and each error mapping. No real network bind.

Out of scope: load tests, fuzzing, end-to-end tests against a real LLM service.

## Out of Scope (Future Work)

- Authentication / authorization
- List/enumerate endpoints
- Readiness probe distinct from `/v1/health`
- Streaming responses
- Rate limiting
