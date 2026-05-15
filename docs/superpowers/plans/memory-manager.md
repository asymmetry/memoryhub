# Memory Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Memory Manager actor sub-system: Storage Actor (filesystem I/O), Index Actor (SQLite metadata + sqlite-vec vectors + FTS5), FileOp Actor (write/read/delete pipelines), Search Actor, and the Memory Manager supervisor that wires them together. LLM embedding is stubbed with a trait returning zero vectors. Synthesizer Actor is out of scope for this plan.

**Architecture:** Actor-based (acktor crate). Memory Manager is a supervisor that owns long-lived Storage and Index child actors. For each incoming request it spawns a short-lived FileOp or Search actor that coordinates between the long-lived children and then terminates. Struct names do NOT use an "Actor" suffix (e.g. `Storage`, not `StorageActor`).

**Tech Stack:** Rust 2024 edition, acktor 1.0, rusqlite 0.39 (bundled, with FTS5 and loadable extensions), sqlite-vec 0.1, sha2 0.11, tokio, serde, chrono, uuid.

**Design spec:** `docs/superpowers/specs/memory-manager-design.md`

> **Status:** Tasks 1–11 (Storage / Index / FileOp / Search / MemoryManager supervisor) were implemented and have since been refactored — the code blocks below describe the historical approach (e.g. `EmbeddingService` trait, generic `MemoryManager<E>`) and no longer match the current code, which uses an `LlmService` actor and a concrete `MemoryManager`. They are kept as a record. Tasks 12–18 (Synthesizer + Session) are the remaining work and target the current codebase.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `Cargo.toml` | Add acktor, rusqlite, sqlite-vec, sha2 |
| Modify | `src/config.rs` | Unify StoreConfig + IndexConfig → MemoryConfig |
| Modify | `tests/data/config.toml` | Update test config to use `[memory]` |
| Modify | `src/lib.rs` | Add `pub mod embedding;` |
| Create | `src/embedding.rs` | EmbeddingService trait + MockEmbeddingService |
| Modify | `src/memory.rs` | Add submodules, remove old MemoryManager trait |
| Create | `src/memory/messages.rs` | All actor message types + Embedding/Chunk/SearchResult |
| Create | `src/memory/chunking.rs` | Line-based overlapping text chunking |
| Create | `src/memory/path.rs` | Path derivation for memory files |
| Modify | `src/memory/content_store.rs` | Storage actor (filesystem I/O) |
| Modify | `src/memory/sqlite.rs` | Index actor (SQLite + FTS5 + sqlite-vec) |
| Create | `src/memory/file_op.rs` | FileOp actor (write/read/delete pipelines) |
| Create | `src/memory/search.rs` | Search actor (query → embed → search) |
| Create | `src/memory/manager.rs` | MemoryManager supervisor actor |
| Modify | `src/main.rs` | Wire MemoryManager into startup |

---

## Task 1: Add dependencies to Cargo.toml

**Files:**
- Modify: `Cargo.toml`

### Steps

- [ ] Add the following to `[dependencies]` in `Cargo.toml` (keep alphabetical order):

```toml
[dependencies]
acktor = "1.0"
anyhow = "1.0"
axum = "0.8"
chrono = { version = "0.4", features = ["serde"] }
dirs = "6.0"
reqwest = { version = "0.13", features = [
    "json",
    "rustls",
], default-features = false }
rusqlite = { version = "0.39", features = ["bundled", "column_decltype"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.11"
sqlite-vec = "0.1"
thiserror = "2.0"
toml = "1.1"
tokio = { version = "1.50", features = ["macros", "rt-multi-thread"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1.23", features = ["v4", "serde"] }
```

- [ ] Run `rtk cargo check` — expect zero errors.

- [ ] Commit: `feat(deps): add acktor, rusqlite, sqlite-vec, sha2`

---

## Task 2: Refactor config (StoreConfig + IndexConfig → MemoryConfig)

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/data/config.toml`

### Steps

- [ ] Write failing tests. Add to the `#[cfg(test)] mod tests` block in `src/config.rs`:

```rust
#[test]
fn test_memory_config_defaults() {
    let config = Config::default();
    assert_eq!(config.memory.memory_dir, "~/.clawchorus/memory");
    assert_eq!(config.memory.db_path, "~/.clawchorus/clawchorus.db");
    assert_eq!(config.memory.chunk_size, 400);
    assert_eq!(config.memory.chunk_overlap, 80);
    assert_eq!(config.memory.temporal_decay_days, 30);
    assert_eq!(config.memory.hybrid_weight, 0.5);
}

#[test]
fn test_memory_config_from_file() {
    let config = Config::from_file(test_config_path()).unwrap();
    assert_eq!(config.memory.memory_dir, "./test_store");
    assert_eq!(config.memory.db_path, ":memory:");
    assert_eq!(config.memory.chunk_size, 400);
    assert_eq!(config.memory.chunk_overlap, 80);
}
```

- [ ] Run `rtk cargo test --lib config::tests` — expect compilation failure because `config.memory` does not exist.

- [ ] Replace `StoreConfig` and `IndexConfig` with a unified `MemoryConfig`:

```rust
/// Memory sub-system configuration (storage + index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Root directory for Markdown memory files on disk.
    pub memory_dir: String,
    /// Path to the SQLite database file.
    pub db_path: String,
    /// Target line count per chunk when splitting content.
    pub chunk_size: usize,
    /// Overlap in lines between consecutive chunks.
    pub chunk_overlap: usize,
    /// Number of days before a memory's score decays to half its original value.
    pub temporal_decay_days: u32,
    /// Default hybrid search weight (`0.0` = pure keyword, `1.0` = pure vector).
    pub hybrid_weight: f32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            memory_dir: "~/.clawchorus/memory".to_string(),
            db_path: "~/.clawchorus/clawchorus.db".to_string(),
            chunk_size: 400,
            chunk_overlap: 80,
            temporal_decay_days: 30,
            hybrid_weight: 0.5,
        }
    }
}
```

- [ ] Update `Config` struct to use `MemoryConfig`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}
```

- [ ] Remove `StoreConfig` and `IndexConfig` structs entirely.

- [ ] Update `tests/data/config.toml` — replace `[store]` and `[index]` with `[memory]`:

```toml
[server]
host = "127.0.0.1"
port = 9090

[memory]
memory_dir = "./test_store"
db_path = ":memory:"
chunk_size = 400
chunk_overlap = 80
temporal_decay_days = 30
hybrid_weight = 0.5

[llm]
provider = "claude"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-6"
embedding_model = "voyage-3"

[agent]
enabled = false
interval_secs = 60
similarity_threshold = 0.75
```

- [ ] Update existing tests that reference `config.store` or `config.index`. The `test_load_from_file` test becomes:

```rust
#[test]
fn test_load_from_file() {
    let config = Config::from_file(test_config_path()).unwrap();
    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.port, 9090);
    assert_eq!(config.memory.memory_dir, "./test_store");
    assert_eq!(config.memory.db_path, ":memory:");
    assert!(!config.agent.enabled);
    assert_eq!(config.agent.interval_secs, 60);
}
```

The `test_defaults` test becomes:

```rust
#[test]
fn test_defaults() {
    let config = Config::default();
    assert_eq!(config.server.host, "0.0.0.0");
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.memory.memory_dir, "~/.clawchorus/memory");
    assert_eq!(config.memory.db_path, "~/.clawchorus/clawchorus.db");
    assert_eq!(config.memory.chunk_size, 400);
    assert_eq!(config.memory.chunk_overlap, 80);
    assert_eq!(config.llm.provider, "claude");
    assert!(config.agent.enabled);
}
```

The `test_partial_config` test becomes:

```rust
#[test]
fn test_partial_config() {
    let toml = r#"
[server]
port = 3000
host = "localhost"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.server.port, 3000);
    assert_eq!(config.memory.memory_dir, "~/.clawchorus/memory");
    assert_eq!(config.memory.chunk_size, 400);
    assert!(config.agent.enabled);
}
```

- [ ] Run `rtk cargo test --lib config::tests` — expect all tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `refactor(config): unify StoreConfig + IndexConfig into MemoryConfig`

---

## Task 3: Define the EmbeddingService trait (LLM stub)

**Files:**
- Create: `src/embedding.rs`
- Modify: `src/lib.rs`

### Steps

- [ ] Create `src/embedding.rs` with only the test:

```rust
//! Embedding service trait and mock implementation.

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embedding_returns_zero_vectors() {
        let mock = MockEmbeddingService::new(128);
        let texts = vec!["hello world".to_string(), "foo bar".to_string()];
        let result = mock.embed_batch(texts).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0.len(), 128);
        assert!(result[0].0.iter().all(|&v| v == 0.0));
        assert_eq!(result[1].0.len(), 128);
    }

    #[test]
    fn mock_dimension() {
        let mock = MockEmbeddingService::new(256);
        assert_eq!(mock.dimension(), 256);
    }
}
```

- [ ] Add `pub mod embedding;` to `src/lib.rs`.

- [ ] Run `rtk cargo test --lib embedding::tests` — expect compilation failure.

- [ ] Implement the trait and mock above the test module in `src/embedding.rs`:

```rust
//! Embedding service trait and mock implementation.
//!
//! The real LLM Service actor will implement [`EmbeddingService`] in a future
//! phase. For now, [`MockEmbeddingService`] returns zero vectors so that the
//! Memory Manager pipeline can be tested end-to-end.

use anyhow::Result;

/// A single embedding vector.
#[derive(Debug, Clone)]
pub struct Embedding(pub Vec<f32>);

/// Trait for obtaining text embeddings.
///
/// The Memory Manager uses this to embed chunks during write and embed
/// queries during search. The real implementation will send messages to the
/// LLM Service actor; the mock returns zero vectors.
pub trait EmbeddingService: Send + Sync {
    /// Embed a batch of text strings, returning one Embedding per input.
    fn embed_batch(
        &self,
        texts: Vec<String>,
    ) -> impl std::future::Future<Output = Result<Vec<Embedding>>> + Send;

    /// The dimensionality of embedding vectors produced by this service.
    fn dimension(&self) -> usize;
}

/// A mock embedding service that returns zero vectors of a fixed dimension.
#[derive(Debug, Clone)]
pub struct MockEmbeddingService {
    dim: usize,
}

impl MockEmbeddingService {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl EmbeddingService for MockEmbeddingService {
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Embedding>> {
        Ok(texts
            .iter()
            .map(|_| Embedding(vec![0.0f32; self.dim]))
            .collect())
    }

    fn dimension(&self) -> usize {
        self.dim
    }
}
```

- [ ] Run `rtk cargo test --lib embedding::tests` — expect pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(embedding): add EmbeddingService trait with Embedding newtype and MockEmbeddingService`

---

## Task 4: Define message types for all actors

**Files:**
- Create: `src/memory/messages.rs`
- Modify: `src/memory.rs`

### Steps

- [ ] Create `src/memory/messages.rs` with only tests:

```rust
//! Message types for the Memory Manager actor hierarchy.

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn storage_write_msg_fields() {
        let msg = StorageWrite {
            rel_path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            content: "hello".to_string(),
        };
        assert_eq!(msg.rel_path, "alice/agent1/daily_note/2026-03-31.md");
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn file_op_write_msg_fields() {
        let msg = FileOpWrite {
            username: "alice".to_string(),
            agent_id: Uuid::new_v4(),
            memory_type: crate::memory::MemoryType::DailyNote,
            filename: "2026-03-31.md".to_string(),
            content: "hello".to_string(),
        };
        assert_eq!(msg.username, "alice");
    }

    #[test]
    fn search_result_fields() {
        let sr = SearchResult {
            path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            start_line: 1,
            end_line: 10,
            score: 0.95,
            snippet: "hello world".to_string(),
        };
        assert!(sr.score > 0.9);
    }

    #[test]
    fn chunk_fields() {
        use crate::embedding::Embedding;
        let chunk = Chunk {
            text: "some text".to_string(),
            start_line: 1,
            end_line: 5,
            embedding: Embedding(vec![0.0; 128]),
        };
        assert_eq!(chunk.start_line, 1);
        assert_eq!(chunk.embedding.0.len(), 128);
    }
}
```

- [ ] Add `pub mod messages;` to `src/memory.rs` (alongside existing submodule declarations).

- [ ] Run `rtk cargo test --lib memory::messages::tests` — expect compilation failure.

- [ ] Implement the message types above the test module:

```rust
//! Message types for the Memory Manager actor hierarchy.
//!
//! Defines all messages exchanged between actors in the Memory Manager
//! sub-system. Each message implements `acktor::Message` with an appropriate
//! `Result` type.

use acktor::message::Message;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::embedding::Embedding;
use crate::memory::MemoryType;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// A chunk of text extracted from a memory file, with its embedding vector.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    /// 1-indexed start line in the original file.
    pub start_line: u32,
    /// 1-indexed end line in the original file.
    pub end_line: u32,
    pub embedding: Embedding,
}

/// A single search result returned from the Index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// Storage messages
// ---------------------------------------------------------------------------

/// Write content to a file at `rel_path`. Creates parent directories.
#[derive(Debug, Clone)]
pub struct StorageWrite {
    pub rel_path: String,
    pub content: String,
}

impl Message for StorageWrite {
    type Result = anyhow::Result<()>;
}

/// Read content from a file at `rel_path`.
#[derive(Debug, Clone)]
pub struct StorageRead {
    pub rel_path: String,
}

impl Message for StorageRead {
    type Result = anyhow::Result<Option<String>>;
}

/// Delete a file at `rel_path`. Idempotent.
#[derive(Debug, Clone)]
pub struct StorageDelete {
    pub rel_path: String,
}

impl Message for StorageDelete {
    type Result = anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// Index messages
// ---------------------------------------------------------------------------

/// Insert or replace a file's chunks in the index.
#[derive(Debug, Clone)]
pub struct IndexInsert {
    pub path: String,
    /// "raw" or "synthesized".
    pub source: String,
    pub size: u64,
    pub model: String,
    pub chunks: Vec<Chunk>,
}

impl Message for IndexInsert {
    type Result = anyhow::Result<()>;
}

/// Delete a file and all its chunks from the index.
#[derive(Debug, Clone)]
pub struct IndexDelete {
    pub path: String,
}

impl Message for IndexDelete {
    type Result = anyhow::Result<()>;
}

/// Search the index using embedding vectors.
#[derive(Debug, Clone)]
pub struct IndexSearch {
    pub embeddings: Vec<Embedding>,
    pub username: String,
    pub agent_id: Uuid,
    pub limit: usize,
}

impl Message for IndexSearch {
    type Result = anyhow::Result<Vec<SearchResult>>;
}

// ---------------------------------------------------------------------------
// FileOp messages (external, from HTTP Server → Memory Manager)
// ---------------------------------------------------------------------------

/// Write a memory file (store + chunk + embed + index).
#[derive(Debug, Clone)]
pub struct FileOpWrite {
    pub username: String,
    pub agent_id: Uuid,
    pub memory_type: MemoryType,
    pub filename: String,
    pub content: String,
}

impl Message for FileOpWrite {
    type Result = anyhow::Result<()>;
}

/// Read a memory file's content.
#[derive(Debug, Clone)]
pub struct FileOpRead {
    pub username: String,
    pub agent_id: Uuid,
    pub memory_type: MemoryType,
    pub filename: String,
}

impl Message for FileOpRead {
    type Result = anyhow::Result<Option<String>>;
}

/// Delete a memory file (index + store).
#[derive(Debug, Clone)]
pub struct FileOpDelete {
    pub username: String,
    pub agent_id: Uuid,
    pub memory_type: MemoryType,
    pub filename: String,
}

impl Message for FileOpDelete {
    type Result = anyhow::Result<()>;
}

/// Search memories for a user+agent.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub username: String,
    pub agent_id: Uuid,
    pub query: String,
}

impl Message for SearchQuery {
    type Result = anyhow::Result<Vec<SearchResult>>;
}
```

- [ ] Run `rtk cargo test --lib memory::messages::tests` — expect all 4 tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(memory): define actor message types for Memory Manager hierarchy`

---

## Task 5: Implement chunking logic

**Files:**
- Create: `src/memory/chunking.rs`
- Modify: `src/memory.rs`

### Steps

- [ ] Create `src/memory/chunking.rs` with only tests:

```rust
//! Content chunking for the Memory Manager.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_content_produces_no_chunks() {
        let chunks = chunk_text("", 400, 80);
        assert!(chunks.is_empty());
    }

    #[test]
    fn whitespace_only_produces_no_chunks() {
        let chunks = chunk_text("   \n\n  \n", 400, 80);
        assert!(chunks.is_empty());
    }

    #[test]
    fn short_content_produces_single_chunk() {
        let content = "Hello world.\nThis is a test.\nThird line.";
        let chunks = chunk_text(content, 400, 80);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 3);
        assert_eq!(chunks[0].text, content);
    }

    #[test]
    fn long_content_produces_overlapping_chunks() {
        let lines: Vec<String> = (1..=100)
            .map(|i| format!("Line number {} has some words in it.", i))
            .collect();
        let content = lines.join("\n");
        let chunks = chunk_text(&content, 20, 5);

        assert!(chunks.len() > 1, "Expected multiple chunks, got {}", chunks.len());
        assert_eq!(chunks[0].start_line, 1);

        let last = chunks.last().unwrap();
        assert_eq!(last.end_line, 100);

        if chunks.len() >= 2 {
            assert!(
                chunks[1].start_line <= chunks[0].end_line,
                "Expected overlap: chunk[1].start_line={} should be <= chunk[0].end_line={}",
                chunks[1].start_line,
                chunks[0].end_line
            );
        }
    }

    #[test]
    fn chunk_text_preserves_all_content() {
        let lines: Vec<String> = (1..=50).map(|i| format!("Line {}", i)).collect();
        let content = lines.join("\n");
        let chunks = chunk_text(&content, 10, 3);

        for line in &lines {
            assert!(
                chunks.iter().any(|c| c.text.contains(line.as_str())),
                "Line '{}' not found in any chunk",
                line
            );
        }
    }
}
```

- [ ] Add `pub mod chunking;` to `src/memory.rs`.

- [ ] Run `rtk cargo test --lib memory::chunking::tests` — expect compilation failure.

- [ ] Implement the chunking function above the test module:

```rust
//! Content chunking for the Memory Manager.
//!
//! Splits Markdown content into overlapping line-based chunks. Each chunk
//! tracks its 1-indexed start and end line numbers in the original file.

/// A raw text chunk before embedding.
#[derive(Debug, Clone)]
pub struct TextChunk {
    pub text: String,
    /// 1-indexed start line in the original file.
    pub start_line: u32,
    /// 1-indexed end line in the original file.
    pub end_line: u32,
}

/// Split `content` into overlapping chunks of `chunk_size` lines
/// with `chunk_overlap` lines of overlap between consecutive chunks.
///
/// Returns an empty vec if `content` is empty or whitespace-only.
pub fn chunk_text(content: &str, chunk_size: usize, chunk_overlap: usize) -> Vec<TextChunk> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines.iter().all(|l| l.trim().is_empty()) {
        return Vec::new();
    }

    let total = lines.len();

    if total <= chunk_size {
        return vec![TextChunk {
            text: content.to_string(),
            start_line: 1,
            end_line: total as u32,
        }];
    }

    let step = if chunk_size > chunk_overlap {
        chunk_size - chunk_overlap
    } else {
        1
    };

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < total {
        let end = (start + chunk_size).min(total);
        let text = lines[start..end].join("\n");
        chunks.push(TextChunk {
            text,
            start_line: (start + 1) as u32,
            end_line: end as u32,
        });

        if end == total {
            break;
        }
        start += step;
    }

    chunks
}
```

- [ ] Run `rtk cargo test --lib memory::chunking::tests` — expect all 5 tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(memory): implement line-based content chunking with overlap`

---

## Task 6: Implement path derivation

**Files:**
- Create: `src/memory/path.rs`
- Modify: `src/memory.rs`

### Steps

- [ ] Create `src/memory/path.rs` with only tests:

```rust
//! Path derivation utilities for the Memory Manager.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryType;
    use uuid::Uuid;

    #[test]
    fn daily_note_path() {
        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = derive_rel_path("alice", agent_id, MemoryType::DailyNote, "2026-03-31.md");
        assert_eq!(
            path,
            "alice/550e8400-e29b-41d4-a716-446655440000/daily_note/2026-03-31.md"
        );
    }

    #[test]
    fn long_term_path() {
        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = derive_rel_path("bob", agent_id, MemoryType::LongTerm, "MEMORY.md");
        assert_eq!(
            path,
            "bob/550e8400-e29b-41d4-a716-446655440000/long_term/MEMORY.md"
        );
    }
}
```

- [ ] Add `pub mod path;` to `src/memory.rs`.

- [ ] Run `rtk cargo test --lib memory::path::tests` — expect compilation failure.

- [ ] Implement above the tests:

```rust
//! Path derivation utilities for the Memory Manager.
//!
//! Layout: `{username}/{agent_id}/{memory_type}/{filename}`

use uuid::Uuid;

use crate::memory::MemoryType;

/// Derive the relative filesystem path for a memory file.
pub fn derive_rel_path(
    username: &str,
    agent_id: Uuid,
    memory_type: MemoryType,
    filename: &str,
) -> String {
    let type_dir = match memory_type {
        MemoryType::DailyNote => "daily_note",
        MemoryType::LongTerm => "long_term",
    };
    format!("{}/{}/{}/{}", username, agent_id, type_dir, filename)
}
```

- [ ] Run `rtk cargo test --lib memory::path::tests` — expect both tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(memory): add path derivation utility`

---

## Task 7: Implement Storage Actor

**Files:**
- Modify: `src/memory/content_store.rs`
- Modify: `src/memory.rs`

### Steps

- [ ] Replace the contents of `src/memory/content_store.rs` with tests only:

```rust
//! Storage Actor — filesystem I/O for plain Markdown files.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::messages::{StorageDelete, StorageRead, StorageWrite};

    #[tokio::test]
    async fn write_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.run("storage-test").unwrap();

        addr.send(StorageWrite {
            rel_path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            content: "Hello world".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let content = addr
            .send(StorageRead {
                rel_path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(content, Some("Hello world".to_string()));
    }

    #[tokio::test]
    async fn read_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.run("storage-test").unwrap();

        let result = addr
            .send(StorageRead {
                rel_path: "nonexistent.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn delete_then_read_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.run("storage-test").unwrap();

        addr.send(StorageWrite {
            rel_path: "to_delete.md".to_string(),
            content: "bye".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(StorageDelete {
            rel_path: "to_delete.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let result = addr
            .send(StorageRead {
                rel_path: "to_delete.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.run("storage-test").unwrap();

        let result = addr
            .send(StorageDelete {
                rel_path: "never_existed.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn overwrite_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.run("storage-test").unwrap();

        addr.send(StorageWrite {
            rel_path: "overwrite.md".to_string(),
            content: "version 1".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(StorageWrite {
            rel_path: "overwrite.md".to_string(),
            content: "version 2".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let content = addr
            .send(StorageRead {
                rel_path: "overwrite.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(content, Some("version 2".to_string()));
    }
}
```

- [ ] Change module visibility in `src/memory.rs` from `pub(crate) mod content_store;` to `pub mod content_store;`.

- [ ] Run `rtk cargo test --lib memory::content_store::tests` — expect compilation failure.

- [ ] Implement the Storage actor above the test module:

```rust
//! Storage Actor — filesystem I/O for plain Markdown files.
//!
//! Dumb path-based filesystem wrapper. Knows nothing about memory types,
//! SQLite, or search. Writes use atomic write-to-temp-then-rename.

use std::path::PathBuf;

use acktor::actor::Actor;
use acktor::message::Handler;
use acktor::Context;
use anyhow::Result;
use tracing::{debug, warn};

use crate::memory::messages::{StorageDelete, StorageRead, StorageWrite};

/// The Storage actor. Owns the root directory path.
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve(&self, rel_path: &str) -> PathBuf {
        self.root.join(rel_path)
    }
}

impl Actor for Storage {
    type Context = Context<Self>;
    type Error = anyhow::Error;
}

impl Handler<StorageWrite> for Storage {
    type Result = Result<()>;

    async fn handle(&mut self, msg: StorageWrite, _ctx: &mut Self::Context) -> Result<()> {
        let path = self.resolve(&msg.rel_path);
        debug!(path = %path.display(), "Storage: writing");

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let tmp_path = path.with_extension("tmp");
        tokio::fs::write(&tmp_path, &msg.content).await?;
        tokio::fs::rename(&tmp_path, &path).await?;

        Ok(())
    }
}

impl Handler<StorageRead> for Storage {
    type Result = Result<Option<String>>;

    async fn handle(&mut self, msg: StorageRead, _ctx: &mut Self::Context) -> Result<Option<String>> {
        let path = self.resolve(&msg.rel_path);
        debug!(path = %path.display(), "Storage: reading");

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

impl Handler<StorageDelete> for Storage {
    type Result = Result<()>;

    async fn handle(&mut self, msg: StorageDelete, _ctx: &mut Self::Context) -> Result<()> {
        let path = self.resolve(&msg.rel_path);
        debug!(path = %path.display(), "Storage: deleting");

        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(path = %path.display(), "Storage: file already gone");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}
```

- [ ] Run `rtk cargo test --lib memory::content_store::tests` — expect all 5 tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(memory): implement Storage actor with atomic writes`

---

## Task 8: Implement Index Actor

**Files:**
- Modify: `src/memory/sqlite.rs`

This is the largest task. The Index actor wraps a `rusqlite::Connection`, creates the schema on startup, and handles `IndexInsert`, `IndexDelete`, and `IndexSearch` messages.

### Steps

- [ ] Replace `src/memory/sqlite.rs` with tests only:

```rust
//! Index Actor — SQLite metadata, vector, and keyword index.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::Embedding;
    use crate::memory::messages::{Chunk, IndexDelete, IndexInsert, IndexSearch};
    use uuid::Uuid;

    fn test_index() -> Index {
        Index::open_in_memory().unwrap()
    }

    #[tokio::test]
    async fn insert_and_search() {
        let index = test_index();
        let (addr, _handle) = index.run("index-test").unwrap();

        addr.send(IndexInsert {
            path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            source: "raw".to_string(),
            size: 100,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "Rust programming language".to_string(),
                start_line: 1,
                end_line: 5,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![0.0; 128])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                limit: 10,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].path, "alice/agent1/daily_note/2026-03-31.md");
    }

    #[tokio::test]
    async fn delete_removes_chunks() {
        let index = test_index();
        let (addr, _handle) = index.run("index-test").unwrap();

        addr.send(IndexInsert {
            path: "alice/agent1/daily_note/temp.md".to_string(),
            source: "raw".to_string(),
            size: 50,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "to be deleted".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(IndexDelete {
            path: "alice/agent1/daily_note/temp.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![0.0; 128])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                limit: 10,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn insert_replaces_existing() {
        let index = test_index();
        let (addr, _handle) = index.run("index-test").unwrap();

        let path = "alice/agent1/daily_note/replace.md".to_string();

        addr.send(IndexInsert {
            path: path.clone(),
            source: "raw".to_string(),
            size: 10,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "version one".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(IndexInsert {
            path: path.clone(),
            source: "raw".to_string(),
            size: 12,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "version two".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![0.0; 128])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                limit: 10,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("version two"));
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let index = test_index();
        let (addr, _handle) = index.run("index-test").unwrap();

        let result = addr
            .send(IndexDelete {
                path: "never/existed.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(result.is_ok());
    }
}
```

- [ ] Run `rtk cargo test --lib memory::sqlite::tests` — expect compilation failure.

- [ ] Implement the Index actor above the test module:

```rust
//! Index Actor — SQLite metadata, vector, and keyword index.
//!
//! Tables: `files` (change tracking), `chunks` (text), `chunks_fts` (FTS5),
//! `chunks_vec` (sqlite-vec embeddings).
//!
//! The actor owns a `rusqlite::Connection` and processes messages sequentially.

use acktor::actor::Actor;
use acktor::message::Handler;
use acktor::Context;
use anyhow::{Context as _, Result};
use chrono::Utc;
use rusqlite::Connection;
use tracing::debug;

use crate::memory::messages::{IndexDelete, IndexInsert, IndexSearch, SearchResult};

/// The Index actor. Owns a SQLite connection.
pub struct Index {
    conn: Connection,
    embedding_dim: usize,
}

impl Index {
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut actor = Self {
            conn,
            embedding_dim: 128,
        };
        actor.init_schema()?;
        Ok(actor)
    }

    pub fn open(path: &str, embedding_dim: usize) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut actor = Self {
            conn,
            embedding_dim,
        };
        actor.init_schema()?;
        Ok(actor)
    }

    fn init_schema(&mut self) -> Result<()> {
        unsafe {
            let _guard = rusqlite::LoadExtensionGuard::new(&self.conn)?;
            sqlite_vec::load(&self.conn)?;
        }

        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS files (
                path       TEXT PRIMARY KEY,
                source     TEXT NOT NULL,
                size       INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id         TEXT PRIMARY KEY,
                path       TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line   INTEGER NOT NULL,
                model      TEXT NOT NULL,
                text       TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_chunks_path ON chunks(path);
            ",
        )?;

        self.conn.execute_batch(
            "
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                text,
                id UNINDEXED,
                path UNINDEXED,
                model UNINDEXED,
                start_line UNINDEXED,
                end_line UNINDEXED,
                content=chunks,
                content_rowid=rowid,
                tokenize='unicode61'
            );
            ",
        )?;

        let vec_sql = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
                chunk_id TEXT PRIMARY KEY,
                embedding float[{}]
            )",
            self.embedding_dim
        );
        self.conn.execute_batch(&vec_sql)?;

        self.conn.execute_batch(
            "
            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, text, id, path, model, start_line, end_line)
                VALUES (new.rowid, new.text, new.id, new.path, new.model, new.start_line, new.end_line);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text, id, path, model, start_line, end_line)
                VALUES ('delete', old.rowid, old.text, old.id, old.path, old.model, old.start_line, old.end_line);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text, id, path, model, start_line, end_line)
                VALUES ('delete', old.rowid, old.text, old.id, old.path, old.model, old.start_line, old.end_line);
                INSERT INTO chunks_fts(rowid, text, id, path, model, start_line, end_line)
                VALUES (new.rowid, new.text, new.id, new.path, new.model, new.start_line, new.end_line);
            END;
            ",
        )?;

        Ok(())
    }

    fn do_insert(&mut self, msg: &IndexInsert) -> Result<()> {
        let tx = self.conn.transaction()?;
        let now = Utc::now().timestamp();

        // Delete old vec entries for this path.
        {
            let mut stmt = tx.prepare("SELECT id FROM chunks WHERE path = ?1")?;
            let ids: Vec<String> = stmt
                .query_map([&msg.path], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for id in &ids {
                tx.execute("DELETE FROM chunks_vec WHERE chunk_id = ?1", [id])?;
            }
        }

        tx.execute("DELETE FROM chunks WHERE path = ?1", [&msg.path])?;
        tx.execute("DELETE FROM files WHERE path = ?1", [&msg.path])?;

        tx.execute(
            "INSERT INTO files (path, source, size, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![msg.path, msg.source, msg.size, now],
        )?;

        for (i, chunk) in msg.chunks.iter().enumerate() {
            let id = format!("{}:{}", msg.path, i);

            tx.execute(
                "INSERT INTO chunks (id, path, start_line, end_line, model, text, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    id,
                    msg.path,
                    chunk.start_line,
                    chunk.end_line,
                    msg.model,
                    chunk.text,
                    now,
                ],
            )?;

            let embedding_blob = vec_to_blob(&chunk.embedding.0);
            tx.execute(
                "INSERT INTO chunks_vec (chunk_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, embedding_blob],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    fn do_delete(&mut self, path: &str) -> Result<()> {
        let tx = self.conn.transaction()?;

        {
            let mut stmt = tx.prepare("SELECT id FROM chunks WHERE path = ?1")?;
            let ids: Vec<String> = stmt
                .query_map([path], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for id in &ids {
                tx.execute("DELETE FROM chunks_vec WHERE chunk_id = ?1", [id])?;
            }
        }

        tx.execute("DELETE FROM chunks WHERE path = ?1", [path])?;
        tx.execute("DELETE FROM files WHERE path = ?1", [path])?;

        tx.commit()?;
        Ok(())
    }

    fn do_search(&self, msg: &IndexSearch) -> Result<Vec<SearchResult>> {
        if msg.embeddings.is_empty() {
            return Ok(Vec::new());
        }

        let query_embedding = &msg.embeddings[0];
        let query_blob = vec_to_blob(&query_embedding.0);
        let path_prefix = format!("{}/", msg.username);

        let mut stmt = self.conn.prepare(
            "SELECT
                cv.chunk_id,
                cv.distance,
                c.path,
                c.start_line,
                c.end_line,
                c.text
             FROM chunks_vec cv
             JOIN chunks c ON c.id = cv.chunk_id
             WHERE cv.embedding MATCH ?1
               AND k = ?2
               AND c.path LIKE ?3
             ORDER BY cv.distance ASC",
        )?;

        let results: Vec<SearchResult> = stmt
            .query_map(
                rusqlite::params![query_blob, msg.limit, format!("{}%", path_prefix)],
                |row| {
                    let distance: f32 = row.get(1)?;
                    Ok(SearchResult {
                        path: row.get(2)?,
                        start_line: row.get(3)?,
                        end_line: row.get(4)?,
                        score: 1.0 - distance,
                        snippet: row.get(5)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(results)
    }
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

impl Actor for Index {
    type Context = Context<Self>;
    type Error = anyhow::Error;
}

impl Handler<IndexInsert> for Index {
    type Result = Result<()>;

    async fn handle(&mut self, msg: IndexInsert, _ctx: &mut Self::Context) -> Result<()> {
        debug!(path = %msg.path, chunks = msg.chunks.len(), "Index: inserting");
        self.do_insert(&msg)
    }
}

impl Handler<IndexDelete> for Index {
    type Result = Result<()>;

    async fn handle(&mut self, msg: IndexDelete, _ctx: &mut Self::Context) -> Result<()> {
        debug!(path = %msg.path, "Index: deleting");
        self.do_delete(&msg.path)
    }
}

impl Handler<IndexSearch> for Index {
    type Result = Result<Vec<SearchResult>>;

    async fn handle(&mut self, msg: IndexSearch, _ctx: &mut Self::Context) -> Result<Vec<SearchResult>> {
        debug!(username = %msg.username, "Index: searching");
        self.do_search(&msg)
    }
}
```

**Important:** `rusqlite::Connection` is `!Send`. If acktor requires `Send` on actors (it does — `Actor: Send + 'static`), wrap the connection:

```rust
use std::sync::{Arc, Mutex};

pub struct Index {
    conn: Arc<Mutex<Connection>>,
    embedding_dim: usize,
}
```

And each handler uses:

```rust
async fn handle(&mut self, msg: IndexInsert, _ctx: &mut Self::Context) -> Result<()> {
    let conn = self.conn.clone();
    let msg = msg;
    tokio::task::spawn_blocking(move || {
        let mut conn = conn.lock().unwrap();
        // ... do_insert logic using &mut conn ...
    }).await?
}
```

Use whichever approach compiles. The direct `Connection` approach is shown above for clarity; switch to `Arc<Mutex>` + `spawn_blocking` if needed.

- [ ] Run `rtk cargo test --lib memory::sqlite::tests` — expect all 4 tests pass.

- [ ] Run `rtk cargo test` — expect all tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(memory): implement Index actor with SQLite, FTS5, and sqlite-vec`

---

## Task 9: Implement MemoryManager supervisor actor

**Files:**
- Create: `src/memory/manager.rs`
- Modify: `src/memory.rs`

The MemoryManager handles FileOp and Search messages directly by coordinating between Storage and Index children. Separate FileOp/Search actors can be extracted later if needed.

### Steps

- [ ] Create `src/memory/manager.rs` with tests only:

```rust
//! Memory Manager Actor — supervisor for the memory sub-system.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::MockEmbeddingService;
    use crate::memory::messages::{FileOpDelete, FileOpRead, FileOpWrite, SearchQuery};
    use crate::memory::MemoryType;
    use std::sync::Arc;
    use uuid::Uuid;

    #[tokio::test]
    async fn full_write_read_delete_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let embed = Arc::new(MockEmbeddingService::new(128));

        let mm = MemoryManager::new(
            dir.path().to_path_buf(),
            ":memory:".to_string(),
            embed,
            400,
            80,
            "mock".to_string(),
            128,
        )
        .unwrap();
        let (addr, _handle) = mm.run("memory-manager").unwrap();

        let agent_id = Uuid::new_v4();

        // Write.
        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "test.md".to_string(),
            content: "Hello from test".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        // Read.
        let content = addr
            .send(FileOpRead {
                username: "alice".to_string(),
                agent_id,
                memory_type: MemoryType::DailyNote,
                filename: "test.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content, Some("Hello from test".to_string()));

        // Delete.
        addr.send(FileOpDelete {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "test.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        // Read after delete.
        let content = addr
            .send(FileOpRead {
                username: "alice".to_string(),
                agent_id,
                memory_type: MemoryType::DailyNote,
                filename: "test.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content, None);
    }

    #[tokio::test]
    async fn search_after_write() {
        let dir = tempfile::tempdir().unwrap();
        let embed = Arc::new(MockEmbeddingService::new(128));

        let mm = MemoryManager::new(
            dir.path().to_path_buf(),
            ":memory:".to_string(),
            embed,
            400,
            80,
            "mock".to_string(),
            128,
        )
        .unwrap();
        let (addr, _handle) = mm.run("memory-manager").unwrap();

        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "notes.md".to_string(),
            content: "Rust programming language is great".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(SearchQuery {
                username: "alice".to_string(),
                agent_id,
                query: "programming".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!results.is_empty());
    }
}
```

- [ ] Add `pub mod manager;` to `src/memory.rs`.

- [ ] Run `rtk cargo test --lib memory::manager::tests` — expect compilation failure.

- [ ] Implement the MemoryManager actor above the test module:

```rust
//! Memory Manager Actor — supervisor for the memory sub-system.
//!
//! Spawns and supervises long-lived Storage and Index child actors.
//! Handles FileOp and Search messages by coordinating between children.

use std::path::PathBuf;
use std::sync::Arc;

use acktor::actor::Actor;
use acktor::address::Address;
use acktor::message::Handler;
use acktor::Context;
use anyhow::Result;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::embedding::EmbeddingService;
use crate::memory::chunking::chunk_text;
use crate::memory::content_store::Storage;
use crate::memory::messages::{
    Chunk, FileOpDelete, FileOpRead, FileOpWrite, IndexDelete, IndexInsert, IndexSearch,
    SearchQuery, SearchResult, StorageDelete, StorageRead, StorageWrite,
};
use crate::memory::path::derive_rel_path;
use crate::memory::sqlite::Index;

pub struct MemoryManager<E: EmbeddingService> {
    storage: Address<Storage>,
    index: Address<Index>,
    embedding: Arc<E>,
    chunk_size: usize,
    chunk_overlap: usize,
    model: String,
    _storage_handle: JoinHandle<()>,
    _index_handle: JoinHandle<()>,
}

impl<E: EmbeddingService> MemoryManager<E> {
    pub fn new(
        memory_dir: PathBuf,
        db_path: String,
        embedding: Arc<E>,
        chunk_size: usize,
        chunk_overlap: usize,
        model: String,
        embedding_dim: usize,
    ) -> Result<Self> {
        let storage = Storage::new(memory_dir);
        let (storage_addr, storage_handle) = storage.run("storage")?;

        let index = if db_path == ":memory:" {
            Index::open_in_memory()?
        } else {
            Index::open(&db_path, embedding_dim)?
        };
        let (index_addr, index_handle) = index.run("index")?;

        info!("MemoryManager: child actors started");

        Ok(Self {
            storage: storage_addr,
            index: index_addr,
            embedding,
            chunk_size,
            chunk_overlap,
            model,
            _storage_handle: storage_handle,
            _index_handle: index_handle,
        })
    }
}

impl<E: EmbeddingService + 'static> Actor for MemoryManager<E> {
    type Context = Context<Self>;
    type Error = anyhow::Error;
}

impl<E: EmbeddingService + 'static> Handler<FileOpWrite> for MemoryManager<E> {
    type Result = Result<()>;

    async fn handle(&mut self, msg: FileOpWrite, _ctx: &mut Self::Context) -> Result<()> {
        let rel_path = derive_rel_path(&msg.username, msg.agent_id, msg.memory_type, &msg.filename);
        debug!(rel_path = %rel_path, "MemoryManager: write");

        // 1. Write to Storage.
        self.storage
            .send(StorageWrite {
                rel_path: rel_path.clone(),
                content: msg.content.clone(),
            })
            .await?
            .await??;

        // 2. Chunk.
        let text_chunks = chunk_text(&msg.content, self.chunk_size, self.chunk_overlap);

        // 3. Embed.
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embeddings = if texts.is_empty() {
            Vec::new()
        } else {
            self.embedding.embed_batch(texts).await?
        };

        // 4. Build chunks with embeddings.
        let chunks: Vec<Chunk> = text_chunks
            .into_iter()
            .zip(embeddings)
            .map(|(tc, emb)| Chunk {
                text: tc.text,
                start_line: tc.start_line,
                end_line: tc.end_line,
                embedding: emb,
            })
            .collect();

        // 5. Insert into Index.
        let result = self
            .index
            .send(IndexInsert {
                path: rel_path.clone(),
                source: "raw".to_string(),
                size: msg.content.len() as u64,
                model: self.model.clone(),
                chunks,
            })
            .await?
            .await?;

        // 6. Rollback on failure.
        if let Err(e) = result {
            error!(rel_path = %rel_path, error = %e, "MemoryManager: index insert failed, rolling back");
            let _ = self
                .storage
                .send(StorageDelete {
                    rel_path: rel_path.clone(),
                })
                .await?
                .await?;
            return Err(e);
        }

        Ok(())
    }
}

impl<E: EmbeddingService + 'static> Handler<FileOpRead> for MemoryManager<E> {
    type Result = Result<Option<String>>;

    async fn handle(&mut self, msg: FileOpRead, _ctx: &mut Self::Context) -> Result<Option<String>> {
        let rel_path = derive_rel_path(&msg.username, msg.agent_id, msg.memory_type, &msg.filename);
        debug!(rel_path = %rel_path, "MemoryManager: read");

        let content = self
            .storage
            .send(StorageRead { rel_path })
            .await?
            .await??;

        Ok(content)
    }
}

impl<E: EmbeddingService + 'static> Handler<FileOpDelete> for MemoryManager<E> {
    type Result = Result<()>;

    async fn handle(&mut self, msg: FileOpDelete, _ctx: &mut Self::Context) -> Result<()> {
        let rel_path = derive_rel_path(&msg.username, msg.agent_id, msg.memory_type, &msg.filename);
        debug!(rel_path = %rel_path, "MemoryManager: delete");

        // Delete from Index first.
        self.index
            .send(IndexDelete {
                path: rel_path.clone(),
            })
            .await?
            .await??;

        // Then delete from Storage.
        self.storage
            .send(StorageDelete { rel_path })
            .await?
            .await??;

        Ok(())
    }
}

impl<E: EmbeddingService + 'static> Handler<SearchQuery> for MemoryManager<E> {
    type Result = Result<Vec<SearchResult>>;

    async fn handle(&mut self, msg: SearchQuery, _ctx: &mut Self::Context) -> Result<Vec<SearchResult>> {
        debug!(username = %msg.username, "MemoryManager: search");

        if msg.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let text_chunks = chunk_text(&msg.query, self.chunk_size, self.chunk_overlap);

        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embeddings = if texts.is_empty() {
            return Ok(Vec::new());
        } else {
            self.embedding.embed_batch(texts).await?
        };

        let results = self
            .index
            .send(IndexSearch {
                embeddings,
                username: msg.username,
                agent_id: msg.agent_id,
                limit: 20,
            })
            .await?
            .await??;

        Ok(results)
    }
}
```

- [ ] Run `rtk cargo test --lib memory::manager::tests` — expect both tests pass.

- [ ] Run `rtk cargo test` — expect all tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(memory): implement MemoryManager supervisor actor`

---

## Task 10: Update memory.rs module structure and remove old trait

**Files:**
- Modify: `src/memory.rs`

### Steps

- [ ] Update `src/memory.rs` module declarations to:

```rust
pub mod chunking;
pub mod content_store;
pub mod manager;
pub mod messages;
pub mod path;
pub mod sqlite;
```

- [ ] Remove the `MemoryManager` trait (lines 131-157 of current `src/memory.rs`). Keep all domain types (`MemoryType`, `MemoryOrigin`, `RawMemory`, `SynthesizedMemory`, `MemoryEntry`).

- [ ] Run `rtk cargo test` — expect all tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `refactor(memory): update module structure and remove old MemoryManager trait`

---

## Task 11: Wire MemoryManager into main.rs

**Files:**
- Modify: `src/main.rs`

### Steps

- [ ] Update `main.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use clawchorus::config;
use clawchorus::embedding::MockEmbeddingService;
use clawchorus::memory::manager::MemoryManager;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = config::Config::load()?;
    info!(
        host = %config.server.host,
        port = config.server.port,
        "ClawChorus starting"
    );
    info!(
        provider = %config.llm.provider,
        model = %config.llm.model,
        "LLM configuration"
    );

    // Resolve memory_dir (expand ~).
    let memory_dir = if config.memory.memory_dir.starts_with("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
        home.join(&config.memory.memory_dir[2..])
    } else {
        PathBuf::from(&config.memory.memory_dir)
    };

    // Resolve db_path (expand ~).
    let db_path = if config.memory.db_path.starts_with("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
        home.join(&config.memory.db_path[2..])
            .to_string_lossy()
            .to_string()
    } else {
        config.memory.db_path.clone()
    };

    let embedding_dim = 128;
    let embedding = Arc::new(MockEmbeddingService::new(embedding_dim));

    let mm = MemoryManager::new(
        memory_dir,
        db_path,
        embedding,
        config.memory.chunk_size,
        config.memory.chunk_overlap,
        config.llm.embedding_model.clone(),
        embedding_dim,
    )?;
    let (_mm_addr, _mm_handle) = mm.run("memory-manager")?;

    info!("Memory Manager started");
    info!("Initialisation complete — HTTP server not yet started");

    tokio::signal::ctrl_c().await?;
    info!("Shutting down");

    Ok(())
}
```

- [ ] Run `rtk cargo check` — expect zero errors.

- [ ] Run `rtk cargo test` — expect all tests pass.

- [ ] Run `rtk cargo fmt`

- [ ] Commit: `feat(main): wire MemoryManager into application startup`

---

## Task 12: Add synthesizer cool-down to `MemoryConfig`

**Files:**
- Modify: `src/memory/config.rs`

- [ ] **Step 1: Add a failing test for the new field's default**

Append inside `mod tests` in `src/memory/config.rs`:

```rust
#[test]
fn default_synthesizer_cooldown_is_300s() {
    let config = MemoryConfig::default();
    assert_eq!(config.synthesizer_cooldown_secs, 300);
}
```

- [ ] **Step 2: Run the test and confirm it fails**

Run: `rtk cargo test --lib memory::config::tests::default_synthesizer_cooldown_is_300s`
Expected: compile error — `synthesizer_cooldown_secs` not found.

- [ ] **Step 3: Add the field to `MemoryConfig` and its default**

Add the field to the struct:

```rust
    /// Seconds to wait after the last `FileChanged` before the Synthesizer
    /// processes its pending set. Zero disables batching (process per event).
    #[serde(default = "default_synthesizer_cooldown_secs")]
    pub synthesizer_cooldown_secs: u64,
```

Add the default helper at module scope:

```rust
fn default_synthesizer_cooldown_secs() -> u64 {
    300
}
```

And add `synthesizer_cooldown_secs: 300,` to the `Default` impl.

- [ ] **Step 4: Run the test and confirm it passes**

Run: `rtk cargo test --lib memory::config`
Expected: all config tests pass.

- [ ] **Step 5: Run `rtk cargo fmt`**

- [ ] **Step 6: Commit**

```bash
git add src/memory/config.rs
git commit -m "feat(config): add synthesizer cool-down to MemoryConfig"
```

---

## Task 13: Add session idle timeout to `LlmConfig`

**Files:**
- Modify: `src/llm/config.rs`

- [ ] **Step 1: Add a failing test for the default**

Append a new test module at the bottom of `src/llm/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_session_idle_timeout_is_600s() {
        let config = LlmConfig::default();
        assert_eq!(config.session_idle_timeout_secs, 600);
    }
}
```

- [ ] **Step 2: Run the test and confirm it fails**

Run: `rtk cargo test --lib llm::config::tests::default_session_idle_timeout_is_600s`
Expected: compile error — field missing.

- [ ] **Step 3: Add the field and default**

Add to `LlmConfig`:

```rust
    /// Seconds a `Session` will sit idle before self-terminating.
    #[serde(default = "default_session_idle_timeout_secs")]
    pub session_idle_timeout_secs: u64,
```

Add the helper at module scope:

```rust
fn default_session_idle_timeout_secs() -> u64 {
    600
}
```

And add `session_idle_timeout_secs: 600,` to the `Default` impl.

- [ ] **Step 4: Run the test and confirm it passes**

Run: `rtk cargo test --lib llm::config`
Expected: pass.

- [ ] **Step 5: Run `rtk cargo fmt`**

- [ ] **Step 6: Commit**

```bash
git add src/llm/config.rs
git commit -m "feat(config): add session idle timeout to LlmConfig"
```

---

## Task 14: Add `FileChanged` and synthesis path helper

**Files:**
- Modify: `src/memory/messages.rs`
- Modify: `src/memory/path.rs`

- [ ] **Step 1: Add a failing test for synthesis path derivation**

Append to `mod tests` in `src/memory/path.rs`:

```rust
#[test]
fn per_user_synthesis_path() {
    let path = derive_synthesis_path(Some("alice"), MemoryType::DailyNote, "2026-05-13.md");
    assert_eq!(path, "alice/_synthesized/daily_note/2026-05-13.md");
}

#[test]
fn cross_user_synthesis_path() {
    let path = derive_synthesis_path(None, MemoryType::LongTerm, "merged.md");
    assert_eq!(path, "_synthesized/long_term/merged.md");
}
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `rtk cargo test --lib memory::path`
Expected: compile error — `derive_synthesis_path` not found.

- [ ] **Step 3: Add `derive_synthesis_path`**

Append to `src/memory/path.rs` (above `#[cfg(test)]`):

```rust
/// Derive the relative filesystem path for a synthesized memory file.
///
/// `username = Some(...)` → `{username}/_synthesized/{memory_type}/{filename}`
/// `username = None`       → `_synthesized/{memory_type}/{filename}`
pub fn derive_synthesis_path(
    username: Option<&str>,
    memory_type: MemoryType,
    filename: &str,
) -> String {
    let type_dir = match memory_type {
        MemoryType::DailyNote => "daily_note",
        MemoryType::LongTerm => "long_term",
    };
    match username {
        Some(u) => format!("{}/_synthesized/{}/{}", u, type_dir, filename),
        None => format!("_synthesized/{}/{}", type_dir, filename),
    }
}
```

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `rtk cargo test --lib memory::path`
Expected: all 4 tests pass.

- [ ] **Step 5: Add the `FileChanged` message**

Append to `src/memory/messages.rs` (after the FileOp messages, before tests):

```rust
// ---------------------------------------------------------------------------
// Synthesizer messages
// ---------------------------------------------------------------------------

/// Fire-and-forget notification that a memory file was written or deleted.
/// Sent from `FileOp` to the `Synthesizer` after a successful index update.
#[derive(Debug, Clone, Message)]
#[result_type(())]
pub struct FileChanged {
    pub rel_path: String,
}
```

Then add a test inside `mod tests`:

```rust
#[test]
fn file_changed_msg_fields() {
    let msg = FileChanged {
        rel_path: "alice/agent1/daily_note/x.md".to_string(),
    };
    assert_eq!(msg.rel_path, "alice/agent1/daily_note/x.md");
}
```

- [ ] **Step 6: Run the tests and confirm they pass**

Run: `rtk cargo test --lib memory::messages`
Expected: pass.

- [ ] **Step 7: Run `rtk cargo fmt`**

- [ ] **Step 8: Commit**

```bash
git add src/memory/path.rs src/memory/messages.rs
git commit -m "feat(memory): add synthesis path helper and FileChanged message"
```

---

## Task 15: Stub `Session` actor and `StartSession` on `LlmService`

**Files:**
- Modify: `src/llm/error.rs`
- Create: `src/llm/session.rs`
- Modify: `src/llm.rs`

- [ ] **Step 1: Add `Actor` variant to `LlmError`**

Edit `src/llm/error.rs`:

```rust
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("LLM provider error: {0}")]
    Provider(String),
    #[error("LLM actor messaging error: {0}")]
    Actor(String),
}
```

- [ ] **Step 2: Create `src/llm/session.rs` with the stub Session actor and tests**

Create the file with this exact content:

```rust
//! Conversation session actor — a child of `LlmService`, one per logical
//! conversation. The current implementation is a stub: every `SendMessage`
//! returns a canned reply.

use std::time::Duration;

use acktor::message::FutureMessageResult;
use acktor::{Actor, Context, Handler, Message, Signal};
use tokio::time::Instant;
use tracing::{trace, warn};

use crate::llm::LlmError;

/// Send a user-authored message into the session. Returns the assistant reply.
#[derive(Debug, Clone, Message)]
#[result_type(Result<String, LlmError>)]
pub struct SendMessage {
    pub content: String,
}

/// Gracefully stop the session.
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), LlmError>)]
pub struct StopSession;

/// Internal tick used by the idle-timeout watchdog.
#[derive(Debug, Clone, Message)]
#[result_type(())]
struct IdleTick;

pub struct Session {
    idle_timeout: Duration,
    last_activity: Instant,
}

impl Session {
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            idle_timeout,
            last_activity: Instant::now(),
        }
    }
}

impl Actor for Session {
    type Context = Context<Self>;
    type Error = LlmError;
}

impl Handler<SendMessage> for Session {
    type Result = Result<String, LlmError>;

    async fn handle(
        &mut self,
        msg: SendMessage,
        _ctx: &mut Self::Context,
    ) -> Result<String, LlmError> {
        trace!("Session SendMessage ({} bytes)", msg.content.len());
        self.last_activity = Instant::now();
        Ok(format!("[stub-reply] {}", msg.content))
    }
}

impl Handler<StopSession> for Session {
    type Result = FutureMessageResult<StopSession>;

    async fn handle(
        &mut self,
        _msg: StopSession,
        ctx: &mut Self::Context,
    ) -> FutureMessageResult<StopSession> {
        let addr = ctx.address().clone();
        FutureMessageResult::new(async move {
            if let Err(e) = addr.do_send(Signal::Terminate).await {
                warn!("Session terminate failed: {}", e);
            }
            Ok(())
        })
    }
}

impl Handler<IdleTick> for Session {
    type Result = ();

    async fn handle(&mut self, _msg: IdleTick, ctx: &mut Self::Context) {
        if self.last_activity.elapsed() >= self.idle_timeout {
            let _ = ctx.address().do_send(Signal::Terminate).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acktor::Actor;

    #[tokio::test]
    async fn send_message_returns_stub_reply() {
        let session = Session::new(Duration::from_secs(60));
        let (addr, _handle) = session.start("session-test").unwrap();

        let reply = addr
            .send(SendMessage {
                content: "hello".to_string(),
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(reply, "[stub-reply] hello");
    }

    #[tokio::test]
    async fn stop_session_terminates() {
        let session = Session::new(Duration::from_secs(60));
        let (addr, handle) = session.start("session-stop").unwrap();

        addr.send(StopSession)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
}
```

- [ ] **Step 3: Wire the session module and add `StartSession` to `LlmService`**

Edit `src/llm.rs`. Add `pub mod session;` next to `pub mod config;` and `pub mod error;`. Add imports:

```rust
use std::time::Duration;

use acktor::message::FutureMessageResult;
use acktor::Address;

use crate::llm::session::Session;
```

Add the new message and handler:

```rust
/// Open a new conversation session. Reply is the spawned `Session` actor's address.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Address<Session>, LlmError>)]
pub struct StartSession;

impl Handler<StartSession> for LlmService {
    type Result = FutureMessageResult<StartSession>;

    async fn handle(
        &mut self,
        _msg: StartSession,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<StartSession> {
        let idle = Duration::from_secs(self.config.session_idle_timeout_secs);
        FutureMessageResult::new(async move {
            let (addr, _handle) = Session::new(idle)
                .start("session")
                .map_err(|e| LlmError::Actor(e.to_string()))?;
            Ok(addr)
        })
    }
}
```

- [ ] **Step 4: Run the session tests**

Run: `rtk cargo test --lib llm::session`
Expected: both tests pass.

- [ ] **Step 5: Run the full test suite to confirm no regressions**

Run: `rtk cargo test`
Expected: all tests pass.

- [ ] **Step 6: Run `rtk cargo fmt`**

- [ ] **Step 7: Commit**

```bash
git add src/llm.rs src/llm/error.rs src/llm/session.rs
git commit -m "feat(llm): add stub Session actor and StartSession protocol"
```

---

## Task 16: Implement the `Synthesizer` actor

**Files:**
- Create: `src/memory/synthesizer.rs`
- Modify: `src/memory.rs`

- [ ] **Step 1: Create `src/memory/synthesizer.rs`**

Create the file with this content:

```rust
//! Synthesizer Actor — long-lived child of MemoryManager.
//!
//! Receives fire-and-forget `FileChanged` notifications, batches them with a
//! cool-down timer, then runs a synthesis pipeline: read source files,
//! converse with LlmService via a Session, chunk + embed the synthesized
//! result, and write it back via Storage + Index under a `_synthesized/`
//! path.

use std::collections::BTreeSet;
use std::time::Duration;

use acktor::message::FutureMessageResult;
use acktor::{Actor, Address, Context, Handler, Message};
use tokio::time::Instant;
use tracing::{error, info, trace, warn};

use crate::llm::session::{SendMessage, StopSession};
use crate::llm::{Embed, LlmService, StartSession};
use crate::memory::{
    MemoryType,
    chunking::chunk_text,
    error::MemoryError,
    index::Index,
    messages::{Chunk, EnsureVecReady, FileChanged, IndexInsert, StorageRead, StorageWrite},
    path::derive_synthesis_path,
    storage::Storage,
};

/// Internal tick used by the cool-down watchdog.
#[derive(Debug, Clone, Message)]
#[result_type(())]
struct CooldownTick;

pub struct Synthesizer {
    storage: Address<Storage>,
    index: Address<Index>,
    llm: Address<LlmService>,
    cooldown: Duration,
    chunk_size: usize,
    chunk_overlap: usize,
    pending: BTreeSet<String>,
    last_event: Option<Instant>,
}

impl Synthesizer {
    pub fn new(
        storage: Address<Storage>,
        index: Address<Index>,
        llm: Address<LlmService>,
        cooldown_secs: u64,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            storage,
            index,
            llm,
            cooldown: Duration::from_secs(cooldown_secs),
            chunk_size,
            chunk_overlap,
            pending: BTreeSet::new(),
            last_event: None,
        }
    }

    async fn process(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let paths: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
        info!(count = paths.len(), "Synthesizer: processing pending set");

        let mut sources = Vec::new();
        for path in &paths {
            match self.storage.send(StorageRead { path: path.clone() }).await {
                Ok(fut) => match fut.await {
                    Ok(Ok(Some(content))) => sources.push((path.clone(), content)),
                    Ok(Ok(None)) => trace!(path = %path, "Synthesizer: source vanished, skipping"),
                    Ok(Err(e)) => warn!(path = %path, error = %e, "Synthesizer: read failed"),
                    Err(e) => warn!(path = %path, error = %e, "Synthesizer: read join failed"),
                },
                Err(e) => warn!(path = %path, error = %e, "Synthesizer: read send failed"),
            }
        }
        if sources.is_empty() {
            return;
        }

        let session_addr = match self.llm.send(StartSession).await {
            Ok(fut) => match fut.await {
                Ok(Ok(addr)) => addr,
                Ok(Err(e)) => { error!("Synthesizer: StartSession failed: {}", e); return; }
                Err(e) => { error!("Synthesizer: StartSession join failed: {}", e); return; }
            },
            Err(e) => { error!("Synthesizer: StartSession send failed: {}", e); return; }
        };

        let prompt = build_synthesis_prompt(&sources);
        let synthesis = match session_addr.send(SendMessage { content: prompt }).await {
            Ok(fut) => match fut.await {
                Ok(Ok(reply)) => reply,
                Ok(Err(e)) => {
                    error!("Synthesizer: SendMessage failed: {}", e);
                    let _ = session_addr.send(StopSession).await;
                    return;
                }
                Err(e) => { error!("Synthesizer: SendMessage join failed: {}", e); return; }
            },
            Err(e) => { error!("Synthesizer: SendMessage send failed: {}", e); return; }
        };
        let _ = session_addr.send(StopSession).await;

        let text_chunks = chunk_text(&synthesis, self.chunk_size, self.chunk_overlap);
        if text_chunks.is_empty() {
            return;
        }
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embed_result = match self.llm.send(Embed { texts }).await {
            Ok(fut) => match fut.await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => { error!("Synthesizer: Embed failed: {}", e); return; }
                Err(e) => { error!("Synthesizer: Embed join failed: {}", e); return; }
            },
            Err(e) => { error!("Synthesizer: Embed send failed: {}", e); return; }
        };

        let chunks: Vec<Chunk> = text_chunks
            .into_iter()
            .zip(embed_result.embeddings)
            .map(|(tc, emb)| Chunk {
                text: tc.text,
                start_line: tc.start_line,
                end_line: tc.end_line,
                embedding: emb,
            })
            .collect();

        let common_user = common_username(&sources);
        let synth_path = derive_synthesis_path(
            common_user.as_deref(),
            MemoryType::LongTerm,
            &synthesis_filename(),
        );

        if let Err(e) = self
            .storage
            .send(StorageWrite { path: synth_path.clone(), content: synthesis.clone() })
            .await
        {
            error!("Synthesizer: storage send failed: {}", e);
            return;
        }

        if let Some(first) = chunks.first() {
            let dim = first.embedding.0.len();
            if let Ok(fut) = self.index.send(EnsureVecReady { dim }).await {
                if let Ok(Err(e)) = fut.await {
                    error!("Synthesizer: EnsureVecReady failed: {}", e);
                    return;
                }
            }
        }
        match self
            .index
            .send(IndexInsert {
                path: synth_path.clone(),
                source: "synthesized".to_string(),
                size: synthesis.len() as u64,
                model: embed_result.model,
                chunks,
            })
            .await
        {
            Ok(fut) => {
                if let Ok(Err(e)) = fut.await {
                    error!(path = %synth_path, error = %e, "Synthesizer: index insert failed");
                }
            }
            Err(e) => error!("Synthesizer: index send failed: {}", e),
        }

        info!(path = %synth_path, "Synthesizer: synthesis written");
    }
}

fn build_synthesis_prompt(sources: &[(String, String)]) -> String {
    let mut out = String::from("Synthesize the following memories:\n\n");
    for (path, content) in sources {
        out.push_str(&format!("## {}\n{}\n\n", path, content));
    }
    out
}

fn common_username(sources: &[(String, String)]) -> Option<String> {
    let first = sources.first()?.0.split('/').next()?.to_string();
    if first == "_synthesized" {
        return None;
    }
    for (path, _) in sources {
        match path.split('/').next() {
            Some(u) if u == first => continue,
            _ => return None,
        }
    }
    Some(first)
}

fn synthesis_filename() -> String {
    format!("synthesis-{}.md", chrono::Utc::now().timestamp())
}

impl Actor for Synthesizer {
    type Context = Context<Self>;
    type Error = MemoryError;
}

impl Handler<FileChanged> for Synthesizer {
    type Result = FutureMessageResult<FileChanged>;

    async fn handle(
        &mut self,
        msg: FileChanged,
        ctx: &mut Self::Context,
    ) -> FutureMessageResult<FileChanged> {
        trace!(rel_path = %msg.rel_path, "Synthesizer: FileChanged");
        self.pending.insert(msg.rel_path);
        self.last_event = Some(Instant::now());

        let addr = ctx.address().clone();
        let cooldown = self.cooldown;
        FutureMessageResult::new(async move {
            tokio::spawn(async move {
                tokio::time::sleep(cooldown).await;
                let _ = addr.do_send(CooldownTick).await;
            });
        })
    }
}

impl Handler<CooldownTick> for Synthesizer {
    type Result = FutureMessageResult<CooldownTick>;

    async fn handle(
        &mut self,
        _msg: CooldownTick,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<CooldownTick> {
        let should_process = matches!(self.last_event, Some(t) if t.elapsed() >= self.cooldown);
        if !should_process {
            return FutureMessageResult::new(async {});
        }
        self.process().await;
        FutureMessageResult::new(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmService;
    use crate::memory::index::Index;
    use crate::memory::messages::StorageWrite;
    use crate::memory::storage::Storage;
    use std::path::PathBuf;

    async fn boot() -> (Address<Storage>, Address<Index>, Address<LlmService>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (storage, _h1) = Storage::new(PathBuf::from(dir.path())).start("s").unwrap();
        let (index, _h2) = Index::open_in_memory().unwrap().start("i").unwrap();
        let (llm, _h3) = LlmService::new(Default::default()).start("l").unwrap();
        (storage, index, llm, dir)
    }

    #[tokio::test]
    async fn synthesizer_writes_synthesis_after_cooldown() {
        let (storage, index, llm, _dir) = boot().await;

        storage
            .send(StorageWrite {
                path: "alice/agent1/daily_note/x.md".to_string(),
                content: "hello world".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let synth = Synthesizer::new(storage.clone(), index, llm, 0, 400, 80);
        let (addr, _handle) = synth.start("synth").unwrap();

        addr.send(FileChanged { rel_path: "alice/agent1/daily_note/x.md".to_string() })
            .await
            .unwrap()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let target_dir = _dir.path().join("alice").join("_synthesized").join("long_term");
        let entries: Vec<_> = std::fs::read_dir(&target_dir)
            .map(|rd| rd.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(!entries.is_empty(), "expected synthesis file under {:?}", target_dir);
    }

    #[test]
    fn common_username_detects_single_owner() {
        let sources = vec![
            ("alice/a/daily_note/x.md".to_string(), String::new()),
            ("alice/b/long_term/y.md".to_string(), String::new()),
        ];
        assert_eq!(common_username(&sources).as_deref(), Some("alice"));
    }

    #[test]
    fn common_username_returns_none_for_mixed_owners() {
        let sources = vec![
            ("alice/a/daily_note/x.md".to_string(), String::new()),
            ("bob/b/long_term/y.md".to_string(), String::new()),
        ];
        assert!(common_username(&sources).is_none());
    }
}
```

- [ ] **Step 2: Wire the module**

Edit `src/memory.rs`, add `pub mod synthesizer;` next to the other module declarations.

- [ ] **Step 3: Run the Synthesizer tests**

Run: `rtk cargo test --lib memory::synthesizer`
Expected: all 3 tests pass.

- [ ] **Step 4: Run the full test suite**

Run: `rtk cargo test`
Expected: all tests pass.

- [ ] **Step 5: Run `rtk cargo fmt`**

- [ ] **Step 6: Commit**

```bash
git add src/memory.rs src/memory/synthesizer.rs
git commit -m "feat(memory): implement Synthesizer actor with cool-down batching"
```

---

## Task 17: Wire `Synthesizer` into `MemoryManager` and `FileOp`

**Files:**
- Modify: `src/memory/manager.rs`
- Modify: `src/memory/file_op.rs`

- [ ] **Step 1: Add a failing integration test in `manager.rs`**

Append a new test inside `mod tests` in `src/memory/manager.rs`:

```rust
#[tokio::test]
async fn write_emits_synthesis_after_cooldown() {
    let dir = tempfile::tempdir().unwrap();

    let mut cfg = test_config(dir.path());
    cfg.synthesizer_cooldown_secs = 0;

    let mm = MemoryManager::new(cfg, test_llm()).unwrap();
    let (addr, _handle) = mm.start("memory-manager").unwrap();

    let agent_id = Uuid::new_v4();
    addr.send(FileOpWrite {
        username: "alice".to_string(),
        agent_id,
        memory_type: MemoryType::DailyNote,
        filename: "first.md".to_string(),
        content: "Some content for synthesis".to_string(),
    })
    .await
    .unwrap()
    .await
    .unwrap()
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let synth_dir = dir.path().join("alice").join("_synthesized").join("long_term");
    let entries: Vec<_> = std::fs::read_dir(&synth_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    assert!(!entries.is_empty(), "expected synthesis file under {:?}", synth_dir);
}
```

- [ ] **Step 2: Run the test and confirm it fails**

Run: `rtk cargo test --lib memory::manager::tests::write_emits_synthesis_after_cooldown`
Expected: compile error (FileOp does not yet accept a synthesizer) **or** assertion failure.

- [ ] **Step 3: Update `FileOp` to accept and notify the Synthesizer**

Edit `src/memory/file_op.rs`. Add `use crate::memory::synthesizer::Synthesizer;` and `use crate::memory::messages::FileChanged;` (and `use tracing::warn;` if absent). Change the struct + `new`:

```rust
pub struct FileOp {
    storage: Address<Storage>,
    index: Address<Index>,
    llm: Address<LlmService>,
    synthesizer: Address<Synthesizer>,
    chunk_size: usize,
    chunk_overlap: usize,
}

impl FileOp {
    pub fn new(
        storage: Address<Storage>,
        index: Address<Index>,
        llm: Address<LlmService>,
        synthesizer: Address<Synthesizer>,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self { storage, index, llm, synthesizer, chunk_size, chunk_overlap }
    }
}
```

In the `Handler<FileOpWrite>` impl, immediately after the successful `IndexInsert` (just before `Ok(())`), fire-and-forget the notification:

```rust
        if let Err(e) = self
            .synthesizer
            .do_send(FileChanged { rel_path: rel_path.clone() })
            .await
        {
            warn!(rel_path = %rel_path, error = %e, "FileOp: synthesizer notify failed");
        }
```

- [ ] **Step 4: Update `MemoryManager` to spawn + supervise the Synthesizer and thread it through to `FileOp`**

Edit `src/memory/manager.rs`. Add `use crate::memory::synthesizer::Synthesizer;`. Add a `synthesizer: Address<Synthesizer>` field and `synthesizer_handle: Option<JoinHandle<()>>` to the struct.

In `MemoryManager::new`, after the Index actor is started:

```rust
        let synthesizer = Synthesizer::new(
            storage_addr.clone(),
            index_addr.clone(),
            llm.clone(),
            config.synthesizer_cooldown_secs,
            config.chunk_size,
            config.chunk_overlap,
        );
        let (synthesizer_addr, synthesizer_handle) = synthesizer.start("synthesizer")?;
```

Add `synthesizer: synthesizer_addr,` and `synthesizer_handle: Some(synthesizer_handle),` to the returned `Self`.

Update `dispatch_file_op` to clone `self.synthesizer.clone()` and pass it to `FileOp::new(...)`.

In `post_stop`, terminate the Synthesizer alongside Storage and Index:

```rust
        if let Some(join_handle) = self.synthesizer_handle.take() {
            if let Err(e) = self.synthesizer.do_send(Signal::Terminate).await {
                warn!("Could not stop synthesizer actor: {}", e.report());
                join_handle.abort();
            }
            if let Err(e) = join_handle.await {
                warn!("Synthesizer actor join error: {}", e);
            }
        }
```

- [ ] **Step 5: Run the new integration test and confirm it passes**

Run: `rtk cargo test --lib memory::manager::tests::write_emits_synthesis_after_cooldown`
Expected: pass.

- [ ] **Step 6: Run the full test suite**

Run: `rtk cargo test`
Expected: all tests pass (existing `full_write_read_delete_cycle` and `search_after_write` continue to pass — they use the default 300s cool-down so synthesis won't fire during their lifetime).

- [ ] **Step 7: Run `rtk cargo fmt`**

- [ ] **Step 8: Commit**

```bash
git add src/memory/manager.rs src/memory/file_op.rs
git commit -m "feat(memory): wire Synthesizer into MemoryManager and FileOp"
```

---

## Task 18: Final verification

- [ ] **Step 1: Run `rtk cargo check`**

Run: `rtk cargo check`
Expected: zero errors.

- [ ] **Step 2: Run the full test suite**

Run: `rtk cargo test`
Expected: all tests pass.

- [ ] **Step 3: Run `rtk cargo fmt`**

- [ ] **Step 4: Confirm `git status` is clean (or only `Cargo.lock` transitive bumps remain)**

---

## Summary

| Task | Component | Status |
|------|-----------|--------|
| 1 | Dependencies | done |
| 2 | MemoryConfig | done (later refactored) |
| 3 | EmbeddingService + MockEmbeddingService | done, then replaced by LlmService actor |
| 4 | Message types | done (later extended) |
| 5 | Chunking | done |
| 6 | Path derivation | done |
| 7 | Storage actor | done |
| 8 | Index actor | done (later: lazy `chunks_vec` + `meta`) |
| 9 | MemoryManager supervisor | done (later: concrete, non-generic) |
| 10 | Module cleanup | done |
| 11 | main.rs wiring | done |
| 12 | Synthesizer cool-down config | pending |
| 13 | Session idle-timeout config | pending |
| 14 | `FileChanged` message + synthesis path | pending |
| 15 | Stub `Session` actor + `StartSession` | pending |
| 16 | Synthesizer actor | pending |
| 17 | Wire Synthesizer into MemoryManager + FileOp | pending |
| 18 | Final verification | pending |

### Critical implementation notes

- **Struct names have no "Actor" suffix**: `Storage`, `Index`, `MemoryManager`, `Synthesizer`, `Session` — not `StorageActor`, etc.
- **acktor message pattern**: `addr.send(msg).await?.await?` — first `.await` sends, second `.await` receives the reply.
- **Fire-and-forget**: use `addr.do_send(msg).await?` when no reply is needed (e.g. `FileChanged` from `FileOp` → `Synthesizer`).
- **`FileChanged` `result_type` is `()`**, not `Result<...>`. The handler still returns `FutureMessageResult<FileChanged>` so it can spawn a delayed `CooldownTick`.
- **`Synthesizer::process` must run inline** inside the `CooldownTick` handler (before returning the future) — `self` cannot be borrowed across an `.await` in the spawned future.
- **Cool-down tests**: existing `MemoryManager` tests rely on the default 300s cool-down so synthesis does not fire during their lifetime. Only the new `write_emits_synthesis_after_cooldown` test sets `synthesizer_cooldown_secs = 0`.
- **`IndexInsert` source field**: `"raw"` for FileOp writes, `"synthesized"` for Synthesizer writes.
