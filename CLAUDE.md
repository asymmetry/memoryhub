# ClawChorus

Centralized memory service for OpenClaw teams: pools raw Markdown from per-user agents, embeds them, and runs a synthesizer for per-user and cross-user summaries.

Design lives in `docs/` — start with `docs/clawchorus-design.md`.

## Layout

Single Rust binary on the `acktor` actor framework (Tokio). `ClawChorus` (`src/manager.rs`) supervises three children:

- `src/http/` — Axum REST, forwards to `MemoryManager`. Routes under `/v1/*`.
- `src/memory/` — `Storage`, `Index` (SQLite + sqlite-vec + FTS5), `Synthesizer`, plus per-request `FileOp` / `SearchOp`.
- `src/llm/` — `LlmService` with `Provider` trait (deepseek / openai / mock), `Embedder`, one `SynthesisTask` per target. Templates in `src/llm/prompts/`.

## Conventions

- Module style: `foo.rs` + `foo/` (no `mod.rs`).
- No `async_trait`; for dyn traits return `Pin<Box<dyn Future>>`.
- Supervisor handlers that await a child reply return `acktor::message::FutureMessageResult<M>`.
- Every `Handler::handle` starts with `debug_trace!("Handle command {:?}", msg);`.
- Logs: embed values in the prose, no `key=value` fields. Format errors with `e.report()` (`acktor::ErrorReport`).
- Timestamps: `chrono::Utc::now()`, not `SystemTime`.
- Memory paths (`src/memory/path.rs`): raw = `{username}/{agent_id}/{filename}` (`/`,`\` flattened to `_`); synthesized = `[{username}/]_synthesized/YYYY-MM-DD-NN.md`.
- `Cargo.toml`: deps sorted alphabetically, only used features.

## Commands

```
cargo fmt    # required after editing any .rs file
cargo clippy
cargo test --features _test    # tests
```
