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
