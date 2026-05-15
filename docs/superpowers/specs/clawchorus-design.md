# ClawChorus Design Spec

## Context

OpenClaw is an open-source personal AI agent that stores memories as local Markdown files. In a team or company setting, each person's OpenClaw builds knowledge in isolation — there is no way to share, aggregate, or synthesize memories across users.

ClawChorus is a centralized memory management and sharing service that pools individual OpenClaw memories into a unified organizational knowledge base. It collects raw memories from agents, embeds them for semantic search, and runs an event-driven synthesizer to produce cross-user insights.

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
- **LLM Service** — Manages conversation sessions as concurrently running child actors. Handles API communication only — no context or prompt engineering. Each provider (DeepSeek, OpenAI, Claude, etc.) is a separate actor implementation sharing the same message protocol. DeepSeek is the first implementation and the default.
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

- Actor-based: each provider (DeepSeek, OpenAI, Claude, etc.) is a separate actor implementation sharing the same message protocol
- DeepSeek is the first implementation and the default provider
- Handles API communication only — no context or prompt engineering (callers own prompt construction)
- Two core capabilities: generate embeddings (handled inline on `LlmService` via a non-blocking future), manage conversation sessions
- Conversations: caller sends StartSession → receives a Session child actor address → sends messages directly to Session → sends StopSession when done. Sessions have an idle timeout for safety.

Component-level design (message protocol, session management, retry/error handling) will be specified separately.

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
