# Memory Manager Actor — Component Design

## Overview

The Memory Manager actor manages memory storage, metadata, and search within MemoryHub. It is a child actor of the Manager supervisor and holds references to the LLM Service actor for embedding operations.

## Actor Hierarchy

```
Memory Manager Actor
  ├── Storage Actor (long-lived) — filesystem I/O
  ├── Index Actor (long-lived) — SQLite metadata + vectors + FTS5
  ├── Synthesizer Actor (long-lived) — event-driven synthesis
  ├── FileOp Actor (per-request, short-lived) — save/get/delete pipeline
  └── Search Actor (per-request, short-lived) — search pipeline
```

- Memory Manager spawns and supervises the three long-lived child actors on startup.
- For each incoming FileOp or Search message, Memory Manager spawns a short-lived child actor, passing it the addresses of Storage, Index, Synthesizer, and LLM Service. The child runs the pipeline and dies.

## External Messages

Messages received by Memory Manager from HTTP Server:

| Message        | Fields                                | Reply              |
| -------------- | ------------------------------------- | ------------------ |
| FileOp::Write  | username, agent_id, filename, content | Ok / Error         |
| FileOp::Read   | username, agent_id, filename          | content / NotFound |
| FileOp::Delete | username, agent_id, filename          | Ok / Error         |
| Search         | username, agent_id, query             | Vec<SearchResult>  |

`filename` is opaque to the memory manager: the agent embeds any sub-path (e.g. `notes/2026-03-31.md`) directly in it. The memory manager flattens it into a single path segment by replacing every `/` and `\` with `_` before touching disk or index. There is no notion of `memory_type` — that concept lives only in OpenClaw.

## Internal Messages

Messages between child actors:

### Storage Messages

| From                       | Message                  | Reply              |
| -------------------------- | ------------------------ | ------------------ |
| FileOp Actor / Synthesizer | Write(rel_path, content) | Ok / Error         |
| FileOp Actor / Synthesizer | Read(rel_path)           | content / NotFound |
| FileOp Actor / Synthesizer | Delete(rel_path)         | Ok / Error         |

### Index Messages

| From                       | Message                                      | Reply               |
| -------------------------- | -------------------------------------------- | ------------------- |
| FileOp Actor / Synthesizer | Insert(path, Vec\<Chunk\>)                   | Ok / Error          |
| FileOp Actor / Synthesizer | Delete(path)                                 | Ok / Error          |
| Search Actor               | Search(Vec\<Embedding\>, username, agent_id) | Vec\<SearchResult\> |

### Synthesizer Messages

| From         | Message               | Reply                  |
| ------------ | --------------------- | ---------------------- |
| FileOp Actor | FileChanged(rel_path) | None (fire-and-forget) |

### LLM Service Messages

| From                                      | Message                                    | Reply            |
| ----------------------------------------- | ------------------------------------------ | ---------------- |
| FileOp Actor / Search Actor / Synthesizer | Embed(Vec\<String\>)                       | Vec\<Embedding\> |
| Synthesizer                               | Synthesize(target, prior_summary, sources) | synthesized text |

`Synthesize` routes to a long-lived per-target synthesis task inside LLM Service that owns prompt engineering and conversation context. See `llm-service-design.md`.

## Pipelines

### Write

1. FileOp Actor derives `rel_path` from (username, agent*id, filename) — `filename` is flattened by replacing `/` and `\` with `*`
2. Send Write(rel_path, content) → Storage
3. Chunk the content (~400 tokens, 80-token overlap)
4. Send Embed(chunks) → LLM Service
5. Send Insert(path, chunks_with_embeddings) → Index
6. If Index fails, send Delete(rel_path) → Storage (rollback)
7. Send FileChanged(rel_path) → Synthesizer (fire-and-forget, only on success)
8. Reply Ok to sender

### Read

1. FileOp Actor derives `rel_path`
2. Send Read(rel_path) → Storage
3. Reply with content or NotFound

### Delete

1. FileOp Actor derives `rel_path`
2. Send Delete(path) → Index (remove metadata + vectors first)
3. Send Delete(rel_path) → Storage (remove file)
4. Reply Ok to sender

### Search

1. Search Actor chunks the query (~400 tokens, 80-token overlap)
2. Send Embed(query_chunks) → LLM Service
3. Send Search(embeddings, username, agent_id) → Index
4. Reply with scored results

### Synthesizer

**On FileChanged(rel_path):** accumulate the path into a pending set. Processing triggers once the cool-down timer expires, then the timer resets.

**Processing (when cool-down expires)** runs two passes. The Synthesizer feeds only the files that changed this cycle to LLM Service; the long-lived synthesis tasks there preserve prior context, so raw memories are never re-sent in bulk. `prior_summary` is the most recent on-disk synthesized file for that target, passed so a cold or just-reset task can reseed without losing state. Each synthesis: Read the source files from Storage, send `Synthesize` to LLM Service, chunk and `Embed` the returned text, then Write it to Storage and Insert it into Index.

- _Per-user pass_ — group the pending paths by user (first path segment, skipping `_synthesized`). For each affected user: read that user's changed files plus the user's current synthesized file (the `prior_summary`), send `Synthesize(User(username), prior_summary, sources)`, and write the result to the current per-user synthesized file (see [Synthesized output files](#synthesized-output-files) for the naming and rotation rules). A failure for one user is logged and skipped; remaining users still proceed.
- _Global pass_ — runs if at least one per-user summary was regenerated. The sources are the per-user summaries produced this cycle; with the current global synthesized file as `prior_summary`, send `Synthesize(Global, prior_summary, sources)` and write the result to the current global synthesized file.

All of a user's raw files feed the same per-user task — there is no per-category synthesis. Finally, clear the pending set.

#### Synthesized output files

Synthesized output is not a single overwritten file. It accumulates over time as date-named files with a size cap:

- Filename for a synthesis run on date `D` is `{D}.md` (e.g. `2026-05-20.md`), in UTC.
- If appending the new content would push the file above the configured cap (`synthesis.max_file_bytes`, default 1 MiB), the writer rolls over to a suffixed file: `{D}-1.md`, `{D}-2.md`, …
- "Current" synthesized file (used both as the write target and as `prior_summary` for the next run) is the lexicographically greatest existing `{date}[-{n}].md` under the target folder, or a fresh `{today}.md` if none exists.
- Each synthesis writes the full new synthesized text — it does not diff or merge with the prior file on disk. Accumulation comes from the sequence of dated files, not from concatenation within one file. The cap is enforced by checking the size of the file the writer would otherwise reuse; if it is already at or above the cap, the writer creates the next suffixed file instead.
- Old files are never deleted by the Synthesizer. Retention/pruning is out of scope for this iteration.

## Storage Actor

Dumb path-based filesystem wrapper. Knows nothing about memory types, SQLite, or search.

- Atomic write-to-temp-then-rename to avoid partial reads
- Read returns None if file does not exist
- Delete is idempotent (succeeds if file already gone)
- `rel_path` is derived by the caller (FileOp Actor), not by Storage

Files on disk are **plain Markdown with no frontmatter**.

### Filesystem Layout

All memory files are stored under the configured `memory_dir` root using this path formula:

- `{memory_dir}/{username}/{agent_id}/{flattened_filename}`

Where:

- `username` — the OpenClaw user who owns the memory
- `agent_id` — UUID of the agent that produced it
- `flattened_filename` — the original filename with every `/` and `\` replaced by `_` (so `notes/2026-03-31.md` becomes `notes_2026-03-31.md`). The agent is responsible for encoding any logical sub-path into this single name; the memory manager does not interpret it.

Synthesized files produced by the Synthesizer live in dated files under `_synthesized` directories at two levels (see [Synthesized output files](#synthesized-output-files) for naming and size-cap rules):

- **Per-user synthesis:** `{memory_dir}/{username}/_synthesized/{date}[-{n}].md`
- **General (cross-user) synthesis:** `{memory_dir}/_synthesized/{date}[-{n}].md`

The `_synthesized` directory distinguishes synthesized output from raw agent memories at both levels.

The `rel_path` passed to Storage messages is always relative to `memory_dir`. Path derivation is the caller's responsibility (FileOp Actor for raw memories, Synthesizer for synthesized files).

## Index Actor

SQLite-based metadata, vector, and keyword index. Follows OpenClaw's schema.

### Tables

**files** — tracks indexed files for change detection:

| Column     | Type             | Notes                               |
| ---------- | ---------------- | ----------------------------------- |
| path       | TEXT PRIMARY KEY | Relative file path                  |
| source     | TEXT NOT NULL    | 'raw' or 'synthesized'              |
| size       | INTEGER NOT NULL | File size in bytes                  |
| updated_at | INTEGER NOT NULL | Unix timestamp of last index update |

**chunks** — indexed text chunks with line numbers:

| Column     | Type             | Notes                                                    |
| ---------- | ---------------- | -------------------------------------------------------- |
| id         | TEXT PRIMARY KEY | `{path}:{chunk_index}`                                   |
| path       | TEXT NOT NULL    | Relative file path (FK to files.path, manually cascaded) |
| start_line | INTEGER NOT NULL | 1-indexed line number in original file                   |
| end_line   | INTEGER NOT NULL | 1-indexed line number in original file                   |
| model      | TEXT NOT NULL    | Embedding model name                                     |
| text       | TEXT NOT NULL    | Actual chunk content                                     |
| updated_at | INTEGER NOT NULL | Unix timestamp                                           |

Chunk ID = `{path}:{chunk_index}` where `chunk_index` is 0-based sequential. Insert always deletes all existing chunks for a path before re-inserting, so stable IDs are not needed.

Embeddings are stored only in `chunks_vec`, not duplicated in this table.

**chunks_fts** — FTS5 virtual table for keyword search:

- Indexed column: `text`
- Unindexed: id, path, model, start_line, end_line
- Tokenizer: `unicode61`

**chunks_vec** — sqlite-vec virtual table for vector search:

- `chunk_id` (PRIMARY KEY) + `embedding` (FLOAT32[dim])
- Created lazily on first insert via `EnsureVecReady { dim }`; `dim` is read from the embedding response.

**meta** — key/value table for runtime invariants:

| Column | Type             | Notes                  |
| ------ | ---------------- | ---------------------- |
| key    | TEXT PRIMARY KEY | e.g. `"embedding_dim"` |
| value  | TEXT NOT NULL    | Stringified value      |

Stores `embedding_dim` so the `chunks_vec` virtual table can be reconstructed on reopen at the same dimension. A mismatched dimension on insert surfaces as `IndexError::DimensionMismatch`.

### Search

Hybrid search combining:

- **Vector search:** cosine distance via sqlite-vec
- **Keyword search:** BM25 via FTS5

Results scoped to (username, agent_id) via path-prefix matching (e.g. `path LIKE '{username}/{agent_id}/%'`). Returned as:

```
SearchResult {
    path: String,
    start_line: u32,
    end_line: u32,
    score: f32,
    snippet: String,
}
```

## Synthesizer Actor

Long-lived child of Memory Manager. FileChanged messages from FileOp Actor accumulate into a pending set, batched by a configurable cool-down timer; on expiry it runs the two-pass synthesis described under [Pipelines → Synthesizer](#synthesizer). It reads changed files and prior summaries from Storage, delegates the synthesis itself to LLM Service via `Synthesize`, and writes the results back through Storage and Index. Prompt engineering and conversation context live in LLM Service, not here.

## Error Types

Per-module error types. No `anyhow` in library code — only in `main.rs`.

| Error Type     | Module              | Covers                                                                                   |
| -------------- | ------------------- | ---------------------------------------------------------------------------------------- |
| `StorageError` | `memory/storage.rs` | Filesystem I/O failures (read, write, delete)                                            |
| `IndexError`   | `memory/index.rs`   | SQLite failures (query, schema, sqlite-vec)                                              |
| `LlmError`     | `llm.rs`            | LLM / embedding API failures                                                             |
| `ConfigError`  | `error.rs`          | Config loading and parsing failures                                                      |
| `MemoryError`  | `memory/error.rs`   | Top-level error for MemoryManager; wraps Storage, Index, and LLM errors via `From` impls |

**Message result types use specific errors:**

- Storage messages (`StorageWrite`, `StorageRead`, `StorageDelete`) → `Result<_, StorageError>`
- Index messages (`IndexInsert`, `IndexDelete`, `IndexSearch`, `EnsureVecReady`) → `Result<_, IndexError>`
- External messages (`FileOpWrite`, `FileOpRead`, `FileOpDelete`, `Search`) → `Result<_, MemoryError>`
- `LlmService` messages (`Embed`, `Synthesize`) → `Result<_, LlmError>`

`MemoryError` converts from child errors automatically via `From` impls, so `?` works naturally in MemoryManager handlers.
