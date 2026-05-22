# HTTP Service — Component Design

## Overview

The HTTP service exposes MemoryHub over a small JSON API consumed by agents. It is a thin Axum-based forwarder: each route deserializes a JSON body, sends one message to the `MemoryManager` actor, and serializes the reply. It owns no business logic and no state beyond the `MemoryManager` address.

A single `HttpServer` actor (child of `Manager`, sibling to `MemoryManager` and `LlmService`) owns the `axum::serve` task and aborts it on stop. It has no message handlers — the handlers are plain async functions holding the `MemoryManager` address as Axum state. Keeping the server inside an actor is purely so it participates in supervision and shared shutdown; the request path itself never goes through the actor's mailbox.

## Endpoints

All request and response bodies are JSON. All routes are nested under `/v1`.

| Method | Path                  | Body                                      | Actor message  | 200 reply                          |
| ------ | --------------------- | ----------------------------------------- | -------------- | ---------------------------------- |
| GET    | `/v1/health`          | —                                         | —              | `{"status":"ok"}`                  |
| POST   | `/v1/memories/write`  | `{username, agent_id, filename, content}` | `FileOpWrite`  | `{}`                               |
| POST   | `/v1/memories/read`   | `{username, agent_id, filename}`          | `FileOpRead`   | `{"content": "..."}` or 404        |
| POST   | `/v1/memories/delete` | `{username, agent_id, filename}`          | `FileOpDelete` | `{}`                               |
| POST   | `/v1/search`          | `{username, agent_id, query}`             | `Search`       | `{"results": [SearchResult, ...]}` |

`filename` is opaque: any `/` or `\` is flattened to `_` server-side, so the agent encodes any logical sub-path into the name. Identity (`username`, `agent_id`) is carried in the body, mirroring the actor message shape — there is no auth or header/path-based identity in this iteration. A read of a missing file maps `Ok(None)` to 404.

## Error Handling

`HttpError` is one enum implementing `IntoResponse`; non-2xx bodies share the shape `{"error": <code>, "message"?: <string>}`.

| Source                              | Status | Code          | `message`?                    |
| ----------------------------------- | ------ | ------------- | ----------------------------- |
| JSON deserialization failure (Axum) | 400    | —             | Axum's default rejection body |
| `FileOpRead` returns `Ok(None)`     | 404    | `not_found`   | no                            |
| `MemoryError` from any actor call   | 500    | `internal`    | yes (error `Display`)         |
| Mailbox/send failure (actor dead)   | 503    | `unavailable` | no                            |

The 500 `message` is the error's `Display` form, which is user-safe by codebase convention — not a debug dump. 400 is not modeled by `HttpError`: it comes from Axum's built-in `Json` extractor rejection and so does not use the unified shape. A `tower_http` `TraceLayer` emits a span per request; there is no retry, rate-limit, or auth middleware in this iteration.

## Out of Scope (Future Work)

- Authentication / authorization
- List/enumerate endpoints, readiness probe distinct from `/v1/health`
- Streaming responses, rate limiting
