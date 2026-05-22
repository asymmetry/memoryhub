# Memory Manager Actor — Component Design

## Overview

The Memory Manager owns memory storage, metadata, and search. It is a child of `MemoryHub` and holds the `LlmService` address for embedding and synthesis.

## Actor Hierarchy

```
Memory Manager
  ├── Storage (long-lived)     — filesystem I/O
  ├── Indexer (long-lived)     — SQLite metadata + vectors + FTS5
  ├── Synthesizer (long-lived) — event-driven cross-user synthesis
  ├── FileOp (per-request)     — write/read/delete pipeline
  └── Search (per-request)     — search pipeline
```

The three long-lived children are spawned on startup. Each incoming request spawns a short-lived `FileOp` or `Search` actor with the addresses it needs; it runs one pipeline and dies. Per-request actors keep each request's state isolated and let the Memory Manager itself stay a thin router.

## Messages

**External** (from HTTP Server): `FileOpWrite`, `FileOpRead`, `FileOpDelete`, `Search`, all carrying `username` + `agent_id` (plus `filename`/`content`/`query`). `filename` is opaque — the agent embeds any sub-path in it, and the Memory Manager flattens `/` and `\` to `_` before touching disk or index.

**Internal**: Storage takes `StorageWrite`/`StorageRead`/`StorageDelete` keyed by `rel_path`. The Indexer takes `IndexInsert`/`IndexDelete` and `IndexSearch` (embeddings + identity scope). The Synthesizer takes `FileChanged(rel_path)`, fire-and-forget. Embedding and synthesis go to `LlmService` (`Embed`, `Synthesize`); `Synthesize` routes to a long-lived per-target task there that owns prompt engineering and context (see `llm-service-design.md`).

## Pipelines

**Write** — derive `rel_path`; write to Storage; chunk the content; `Embed` the chunks; `IndexInsert`. If indexing fails, delete the file from Storage (rollback) so disk and index never diverge. On success, notify the Synthesizer with `FileChanged` (fire-and-forget), then reply.

**Read** — derive `rel_path`; read from Storage; reply with content or NotFound.

**Delete** — remove from the Indexer **first**, then from Storage. Index-first means a crash mid-delete leaves an orphaned file (harmless, re-indexable) rather than an index entry pointing at a missing file.

**Search** — chunk the query; `Embed`; `IndexSearch` scoped to the caller's `(username, agent_id)`; reply with scored results.

**Synthesizer** — `FileChanged` paths accumulate into a pending set, batched by a cool-down timer. On expiry it runs two passes, feeding only the files changed this cycle (the long-lived synthesis tasks preserve prior context, so raw memories are never re-sent in bulk):

- _Per-user pass_ — group pending paths by user; for each, `Synthesize(User, prior_summary, sources)` from the user's changed files plus their current synthesized file, and write the result back. One user's failure is logged and skipped; the rest proceed.
- _Global pass_ — runs only if at least one per-user summary was regenerated; its sources are those summaries, with the current global synthesized file as `prior_summary`.

`prior_summary` is the most recent on-disk synthesized file for the target, passed so a cold or just-reset task can reseed without losing state.

## Storage & Filesystem Layout

Storage is a dumb path-based wrapper — no knowledge of memory types, SQLite, or search. Writes are atomic (temp-then-rename) to avoid partial reads; reads return None for missing files; deletes are idempotent. Files are plain Markdown with no frontmatter. `rel_path` is always derived by the caller (FileOp for raw memories, Synthesizer for synthesized files), never by Storage.

Under the configured `memory_dir`:

- Raw memory: `{username}/{agent_id}/{flattened_filename}`
- Per-user synthesis: `{username}/_synthesized/{date}-{NN}.md`
- Cross-user synthesis: `_synthesized/{date}-{NN}.md`

The `_synthesized` directory separates synthesized output from raw memories at both levels.

### Synthesized output files

Synthesized output accumulates as dated files rather than a single overwritten file, with a size cap:

- A run on date `D` writes `{D}-{NN}.md` (`NN` is a zero-padded 2-digit sequence starting at `01`, UTC).
- The "current" file — the write target and the next run's `prior_summary` — is the lexicographically greatest existing `{date}-{NN}.md`, or a fresh `{today}-01.md` if none exists.
- If that file is already at or above `synthesis.max_file_bytes` (default 1 MiB), the writer rolls to the next suffix instead.
- Each run writes the full synthesized text; accumulation comes from the sequence of dated files, not concatenation within a file. Old files are never pruned (out of scope).

## Index

SQLite-based metadata, vector, and keyword index over four logical stores:

- **files** — one row per indexed file (path, source `raw`/`synthesized`, size, updated-at) for change detection.
- **chunks** — text chunks with 1-indexed `start_line`/`end_line`, model, and text. Chunk id is `{path}#{chunk_index}` (0-based). Insert deletes all of a path's existing chunks first and re-inserts, so chunk ids need not be stable.
- **chunks_fts** — FTS5 virtual table over chunk text (unicode61 tokenizer) for BM25 keyword search.
- **chunks_vec** — sqlite-vec virtual table holding the embeddings; created lazily at the embedding dimension read from the first response. The dimension is persisted in a small key/value `meta` table so the vector table can be reconstructed on reopen; a mismatched dimension on insert is an error.

Search is hybrid (sqlite-vec cosine + FTS5 BM25), scoped to `(username, agent_id)` by path-prefix match, returning `path`, `start_line`, `end_line`, `score`, `snippet`. Chunking is ~400 tokens with 80-token overlap.

## Errors

Per-module error types, no `anyhow` in library code (only `main.rs`). `StorageError`, `IndexError`, and `MemoryError` live together in `memory/error.rs`; `ConfigError` in `error.rs`; `LlmError` in `llm/error.rs`. `MemoryError` is the top-level type for the Memory Manager and converts from the child errors via `From`, so `?` composes naturally; the per-message reply types use the specific child error.
