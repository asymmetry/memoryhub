# MemoryHub Design Spec

## Context

Personal AI agents store memories as local Markdown files, so in a team each person's agent builds knowledge in isolation — no way to share or aggregate across users. MemoryHub is a centralized service that pools those memories into a shared knowledge base: it collects raw memories from agents, embeds them for semantic search, and runs an event-driven synthesizer to produce per-user and cross-user summaries.

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

- **HTTP Server** — REST front door; a thin forwarder, one `MemoryManager` message per request. See `http-server-design.md`.
- **Memory Manager** — owns storage, the SQLite index, and the synthesizer; runs the read/write/search pipelines. See `memory-manager-design.md`.
- **LLM Service** — all outbound model traffic: embedding and synthesis. See `llm-service-design.md`.
- **MemoryHub** — top-level supervisor; spawns and monitors the children, runs no business logic. See `supervisor-design.md`.

## Design Decisions

- **Actor model (acktor).** Each sub-system is an actor with its own mailbox and no shared mutable state, isolating failure domains and making supervision the single lifecycle mechanism.
- **Embed on save.** Every memory is embedded and indexed when written, so it is immediately searchable — no "unprocessed" state to reconcile later.
- **Vector search (hybrid planned).** Semantic retrieval via sqlite-vec cosine similarity. The FTS5 keyword index is maintained alongside it so BM25 fusion can be added later, but search is vector-only today.
- **Plain Markdown on disk; metadata in SQLite.** Files carry no frontmatter; the index can be rebuilt without rewriting memories.
- **Synthesis split from embedding.** Separated end-to-end so a deployment can pair a chat-only vendor (e.g. DeepSeek) with a different embeddings vendor (e.g. OpenAI).

## Command-line interface

`clap` parses arguments before startup; there are no subcommands — running `memoryhub` always boots the supervisor and server. The `Cli` struct (`memoryhub/src/cli.rs`) exposes flags only:

| Flag                  | Effect                                                                 |
| --------------------- | ---------------------------------------------------------------------- |
| `--base-dir PATH`     | Root directory for all MemoryHub data (see Base directory).            |
| `-c`, `--config PATH` | Config file to load. Defaults to `{base}/config.toml`.                 |
| `--host HOST`         | Override `server.host` after the config is loaded.                     |
| `--port PORT`         | Override `server.port` after the config is loaded.                     |
| `--log-level FILTER`  | Tracing filter (e.g. `info`, `memoryhub=debug`); overrides `RUST_LOG`. |
| `--version`, `--help` | Provided automatically by `clap`.                                      |

With `--config` the named file must exist; without it the default path is read, falling back to built-in defaults (with a warning) when absent. Precedence is CLI flag > config file > built-in default (`apply_overrides` writes `--host`/`--port` into `config.server` only when present). The `EnvFilter` is built from `--log-level`, else `RUST_LOG`.

## Base directory

All on-disk state lives under one base directory, resolved by `config::base_dir` as `--base-dir` > `$MEMORYHUB_HOME` > `~/.memoryhub`. It is established before the config file is read (the default config path derives from it), so it can't live in the config. Setting `--base-dir` or `MEMORYHUB_HOME` needs no home directory — the primary Docker knob, where `MEMORYHUB_HOME` points at a mounted volume.

The data paths default relative to the base, so defaults are unchanged when the base is `~/.memoryhub`:

| Setting             | Default (relative) | Resolves to             |
| ------------------- | ------------------ | ----------------------- |
| config file         | `config.toml`      | `{base}/config.toml`    |
| `memory.memory_dir` | `memory`           | `{base}/memory`         |
| `memory.db_path`    | `memoryhub.db`     | `{base}/memoryhub.db`   |
| `llm.prompts_dir`   | `prompts`          | `{base}/prompts`        |

`resolve_paths` (on `MemoryConfig` and `LlmConfig`) leaves the `:memory:` sentinel and absolute paths untouched, expands a `~/…` prefix to the home directory (back-compat), and joins any other relative path onto the base dir.
