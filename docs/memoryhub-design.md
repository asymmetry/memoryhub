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
| `--base-dir PATH`     | Root directory for all MemoryHub data. See "Base directory" below.     |
| `-c`, `--config PATH` | Config file to load. Defaults to `{base}/config.toml`.                 |
| `--host HOST`         | Override `server.host` after the config is loaded.                     |
| `--port PORT`         | Override `server.port` after the config is loaded.                     |
| `--log-level FILTER`  | Tracing filter (e.g. `info`, `memoryhub=debug`); overrides `RUST_LOG`. |
| `--version`, `--help` | Provided automatically by `clap`.                                      |

**Config-path semantics.** `Config::load` takes the chosen path. With `--config` the named file _must_ exist — a missing file is an error, since the operator asked for it explicitly. Without the flag, the default path (`{base}/config.toml`) is read, falling back to built-in defaults (with a warning) when absent — the current behavior.

**Override precedence.** `main` loads the config, then calls `cli.apply_overrides(&mut config)`, which writes `--host`/`--port` into `config.server` only when present. So precedence is: CLI flag > config file > built-in default.

**Logging.** The `EnvFilter` is built from `--log-level` when given, otherwise from `RUST_LOG` via `from_default_env()`.

## Base directory

All on-disk state lives under a single base directory, resolved by `config::base_dir` with this precedence:

```
--base-dir flag  >  $MEMORYHUB_HOME  >  ~/.memoryhub
```

The base dir is established before the config file is read (the default config path derives from it), so it cannot live in the config file itself. When `--base-dir` or `MEMORYHUB_HOME` is set, no home directory is required — this is the primary deployment knob for the Docker image, where `MEMORYHUB_HOME` points at a mounted volume.

The four data paths default to names relative to the base dir, so the effective defaults are unchanged when the base is `~/.memoryhub`:

| Setting             | Default (relative) | Resolves to             |
| ------------------- | ------------------ | ----------------------- |
| config file         | `config.toml`      | `{base}/config.toml`    |
| `memory.memory_dir` | `memory`           | `{base}/memory`         |
| `memory.db_path`    | `memoryhub.db`     | `{base}/memoryhub.db`   |
| `llm.prompts_dir`   | `prompts`          | `{base}/prompts`        |

**Path resolution** (`MemoryConfig::resolve_paths` and `LlmConfig::resolve_paths`, both taking the base dir): the SQLite `:memory:` sentinel and absolute paths are left untouched; a `~/…` prefix expands to the home directory (kept for back-compat); any other relative path is joined onto the base dir.
