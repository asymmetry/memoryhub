# MemoryHub Design Spec

## Context

OpenClaw is an open-source personal AI agent that stores memories as local Markdown files. In a team or company setting, each person's OpenClaw builds knowledge in isolation — there is no way to share, aggregate, or synthesize memories across users.

MemoryHub is a centralized memory management and sharing service that pools individual OpenClaw memories into a unified organizational knowledge base. It collects raw memories from agents, embeds them for semantic search, and runs an event-driven synthesizer to produce cross-user insights.

## Architecture

Single Rust binary built on the `acktor` actor framework (Tokio-based). The Manager actor is the supervisor for three child actors:

```
                      +-----------+
                      |  Manager  |
                      +-----+-----+
                            |
          +-----------------+----------------+
          |                 |                |
   +------v------+ +--------v-------+ +------v------+
   | HTTP Server | | Memory Manager | | LLM Service |
   |    Actor    | |      Actor     | |    Actor    |
   +-------------+ +----------------+ +-------------+
```

### Sub-systems

- **Memory Manager** — Manages memory storage, metadata, and search. Spawns a child actor for each new memory file received or each query request. Contains long-lived child actors: Storage (filesystem I/O), Index (SQLite), and Synthesizer (cross-user synthesis). Holds a reference to the LLM Service actor for embedding and LLM operations.
- **LLM Service** — Two separated capabilities, each a child actor: embedding (`Embedder`) and document synthesis (one long-lived `SynthesisTask` per target, owning prompt engineering and context). Provider HTTP details sit behind a `Provider` trait; DeepSeek is the first implementation and the default.
- **HTTP Server** — Axum-based endpoints. Receives external requests from OpenClaw agents, forwards all requests directly to the Memory Manager actor.
- **Manager** — Supervisor for the three child actors. Spawns, monitors, and restarts child actors as needed. Does not run business logic itself.

### Design Decisions

- **Actor model (acktor crate):** Each sub-system is an actor with its own mailbox, processing messages sequentially. Actors communicate exclusively through message-passing — no shared mutable state. The Manager supervises all child actors using acktor's supervision support.
- **All memories are embedded on save** and immediately searchable via vector similarity. There is no "unprocessed" state.
- **Memory Manager** holds a reference to the LLM Service actor for embedding and LLM operations.
- **Manager is purely supervisory** — it spawns and monitors child actors but does not run business logic itself.

## Memory Manager Sub-system

Manages memory storage, metadata, and search. Contains child actors:

- **Storage Actor** — filesystem I/O for plain Markdown files (store/retrieve)
- **Index Actor** — SQLite database maintenance and query (metadata, sqlite-vec vectors, FTS5 keyword index)
- **Synthesizer Actor** — event-driven synthesis of memories across users via the LLM Service actor

### Key Design Points

- Files on disk are **plain Markdown with no frontmatter**
- The id → file path mapping lives exclusively in SQLite
- Hybrid search: sqlite-vec cosine similarity + FTS5 BM25 keyword matching
- Chunking follows OpenClaw's approach (~400 tokens per chunk, 80-token overlap)
- Query embedding handled by calling the LLM Service actor

Component-level design (child actor structure, SQLite schema, filesystem layout, coordination logic, error handling) will be specified in a separate document before implementation.

## LLM Sub-system

- `LlmService` actor with one provider behind a `Provider` trait (DeepSeek, OpenAI, Claude, etc.); DeepSeek is the first implementation and the default
- Two clearly separated capabilities, each its own child actor: **generate embeddings** (`Embedder`, handled via a non-blocking future) and **synthesize documents** (one long-lived `SynthesisTask` actor per synthesis target)
- Synthesis owns its prompt engineering and conversation context; prompts are Markdown templates loaded from disk and hot-reloaded
- Each `SynthesisTask` preserves context across cool-down cycles so only changed content is sent each cycle; it idle-terminates and reseeds from the prior summary

Component-level design is specified in `llm-service-design.md`.

## HTTP Sub-system

- Axum-based REST API
- Endpoints for pushing memories, querying/searching, ingesting from workspaces, syncing
- Forwards all requests directly to the Memory Manager actor via message-passing

Component-level design (endpoints, request/response schemas) will be specified separately.

## Manager

- Supervisor for HTTP Server, Memory Manager, and LLM Service child actors
- Spawns, monitors, and restarts child actors as needed
- Does not run business logic itself — purely supervisory

Component-level design will be specified separately.
