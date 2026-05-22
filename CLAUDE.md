# MemoryHub

Centralized memory service for teams: pools raw Markdown from per-user agents, embeds them, and runs a synthesizer for per-user and cross-user summaries.

Design lives in `docs/` — start with `docs/memoryhub-design.md`.

## Layout

Cargo workspace; the `memoryhub` crate lives in `memoryhub/`. Sibling top-level folders: `docker/` (build & deployment assets) and `agents/` (plugins for Claude Code, OpenClaw, Codex, etc.).

Single Rust binary on the `acktor` actor framework (Tokio). `MemoryHub` (`memoryhub/src/supervisor.rs`) supervises three children:

- `memoryhub/src/http/` — Axum REST, forwards to `MemoryManager`. Routes under `/v1/*`.
- `memoryhub/src/memory/` — `Storage`, `Index` (SQLite + sqlite-vec + FTS5), `Synthesizer`, plus per-request `FileOp` / `SearchOp`.
- `memoryhub/src/llm/` — `LlmService` with `Provider` trait (deepseek / openai / mock), `Embedder`, one `SynthesisTask` per target. Templates in `memoryhub/src/llm/prompts/`.

## Conventions

- Module style: `foo.rs` + `foo/` (no `mod.rs`).
- No `async_trait`; for dyn traits return `Pin<Box<dyn Future>>`.
- Supervisor handlers that await a child reply return `acktor::message::FutureMessageResult<M>`.
- Every `Handler::handle` starts with `debug_trace!("Handle command {:?}", msg);`.
- Logs: embed values in the prose, no `key=value` fields. Format errors with `e.report()` (`acktor::ErrorReport`).
- Timestamps: `chrono::Utc::now()`, not `SystemTime`.
- Memory paths (`memoryhub/src/memory/path.rs`): raw = `{username}/{agent_id}/{filename}` (`/`,`\` flattened to `_`); synthesized = `[{username}/]_synthesized/YYYY-MM-DD-NN.md`.
- `Cargo.toml`: deps sorted alphabetically, only used features.

## Commands

```
cargo fmt    # required after editing any .rs file
cargo clippy
cargo test   # tests
```
