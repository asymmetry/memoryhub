# MemoryHub Design Spec

## Context

Personal AI agents store memories as local Markdown files. In a team or company setting, each person's agent builds knowledge in isolation — there is no way to share, aggregate, or synthesize memories across users.

MemoryHub is a centralized memory management and sharing service that pools individual agents' memories into a unified organizational knowledge base. It collects raw memories from agents, embeds them for semantic search, and runs an event-driven synthesizer to produce cross-user insights.

## Architecture

Single Rust binary built on the `acktor` actor framework (Tokio). The top-level `MemoryHub` actor (named after the crate) supervises three child actors:

```
                      +-----------+
                      | MemoryHub |
                      +-----+-----+
                            |
          +-----------------+----------------+
          |                 |                |
   +------v------+ +--------v-------+ +------v------+
   | HTTP Server | | Memory Manager | | LLM Service |
   |    Actor    | |      Actor     | |    Actor    |
   +-------------+ +----------------+ +-------------+
```

- **HTTP Server** — REST front door. A thin forwarder: turns each request into one `MemoryManager` message and serializes the reply. See `http-server-design.md`.
- **Memory Manager** — owns storage, the SQLite index, and the synthesizer; runs the read/write/search pipelines. See `memory-manager-design.md`.
- **LLM Service** — all outbound model traffic: embedding and document synthesis. See `llm-service-design.md`.
- **MemoryHub** — top-level supervisor; spawns and monitors the three children, runs no business logic. See `supervisor-design.md`.

## Design Decisions

- **Actor model (acktor).** Each sub-system is an actor with its own mailbox, processing messages sequentially and communicating only by message-passing — no shared mutable state. This isolates failure domains and makes supervision the single mechanism for lifecycle and shutdown.
- **Embed on save.** Every memory is embedded and indexed when written, so it is immediately searchable. There is no "unprocessed" state to reconcile later.
- **Hybrid search.** Vector similarity (sqlite-vec) is combined with BM25 keyword matching (FTS5) so results are robust to both semantic and literal queries.
- **Plain Markdown on disk; metadata in SQLite.** Files carry no frontmatter; the id → path mapping and all index data live in SQLite. The filesystem stays human-readable and the index can be rebuilt without rewriting memories.
- **Synthesis split from chat plumbing.** Embedding and synthesis are separated end-to-end so a deployment can pair a chat-only vendor (e.g. DeepSeek) with a different embeddings vendor (e.g. OpenAI).

## Command-line interface

The binary parses its arguments with `clap` (derive) before starting. There are no subcommands — running `memoryhub` always boots the supervisor and server. The `Cli` struct lives in `memoryhub/src/cli.rs` and exposes flags only:

| Flag                  | Effect                                                                 |
| --------------------- | ---------------------------------------------------------------------- |
| `-c`, `--config PATH` | Config file to load. Defaults to `~/.memoryhub/config.toml`.           |
| `--host HOST`         | Override `server.host` after the config is loaded.                     |
| `--port PORT`         | Override `server.port` after the config is loaded.                     |
| `--log-level FILTER`  | Tracing filter (e.g. `info`, `memoryhub=debug`); overrides `RUST_LOG`. |
| `--version`, `--help` | Provided automatically by `clap`.                                      |

**Config-path semantics.** `Config::load` takes the chosen path. With `--config` the named file _must_ exist — a missing file is an error, since the operator asked for it explicitly. Without the flag, the default path is read, falling back to built-in defaults (with a warning) when absent — the current behavior.

**Override precedence.** `main` loads the config, then calls `cli.apply_overrides(&mut config)`, which writes `--host`/`--port` into `config.server` only when present. So precedence is: CLI flag > config file > built-in default.

**Logging.** The `EnvFilter` is built from `--log-level` when given, otherwise from `RUST_LOG` via `from_default_env()`.
