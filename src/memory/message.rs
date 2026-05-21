//! Message types for the Memory Manager actor hierarchy.
//!
//! Defines all messages exchanged between actors in the Memory Manager
//! sub-system. Each message uses `#[derive(Message)]` from acktor
//! with `#[result_type(...)]` to specify the reply type.

use acktor::Message;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::llm::Embedding;
use crate::memory::error::{IndexError, MemoryError, StorageError};

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// A chunk of text extracted from a memory file, with its embedding vector.
#[derive(Debug)]
pub struct Chunk {
    pub text: String,
    /// Start line in the original file (1-indexed).
    pub start_line: u32,
    /// End line in the original file (1-indexed).
    pub end_line: u32,
    pub embedding: Embedding,
}

/// A single search result returned from the Indexer.
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

/// Write content to a file at `path`. Creates parent directories.
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), StorageError>)]
pub struct StorageWrite {
    pub path: String,
    pub content: String,
}

/// Read content from a file at `path`.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Option<String>, StorageError>)]
pub struct StorageRead {
    pub path: String,
}

/// Delete a file at `path`. Idempotent.
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), StorageError>)]
pub struct StorageDelete {
    pub path: String,
}

// ---------------------------------------------------------------------------
// Indexer messages
// ---------------------------------------------------------------------------

/// Insert or replace a file's chunks in the index.
#[derive(Debug, Message)]
#[result_type(Result<(), IndexError>)]
pub struct IndexInsert {
    pub path: String,
    /// "raw" or "synthesized".
    pub source: String,
    pub size: u64,
    pub model: String,
    pub chunks: Vec<Chunk>,
}

/// Delete a file and all its chunks from the index.
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), IndexError>)]
pub struct IndexDelete {
    pub path: String,
}

/// Ensure the `chunks_vec` virtual table exists for `dim`-sized vectors.
/// Creates it on first call and persists `dim` to the `meta` table.
/// Returns `DimensionMismatch` if a different dimension was previously stored.
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), IndexError>)]
pub struct EnsureVecReady {
    pub dim: usize,
}

/// Search the index using embedding vectors.
#[derive(Debug, Message)]
#[result_type(Result<Vec<SearchResult>, IndexError>)]
pub struct IndexSearch {
    pub embeddings: Vec<Embedding>,
    pub username: String,
    pub agent_id: Uuid,
    pub limit: usize,
}

// ---------------------------------------------------------------------------
// FileOp messages (external, from HTTP Server → Memory Manager)
// ---------------------------------------------------------------------------

/// Write a memory file (store + chunk + embed + index).
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), MemoryError>)]
pub struct FileOpWrite {
    pub username: String,
    pub agent_id: Uuid,
    pub filename: String,
    pub content: String,
}

/// Read a memory file's content.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Option<String>, MemoryError>)]
pub struct FileOpRead {
    pub username: String,
    pub agent_id: Uuid,
    pub filename: String,
}

/// Delete a memory file (index + store).
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), MemoryError>)]
pub struct FileOpDelete {
    pub username: String,
    pub agent_id: Uuid,
    pub filename: String,
}

/// Search memories for a user+agent.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Vec<SearchResult>, MemoryError>)]
pub struct Search {
    pub username: String,
    pub agent_id: Uuid,
    pub query: String,
}

// ---------------------------------------------------------------------------
// Synthesizer messages
// ---------------------------------------------------------------------------

/// Fire-and-forget notification that a memory file was written or deleted.
///
/// Sent from `FileOp` to the `Synthesizer` after a successful index update.
#[derive(Debug, Clone, Message)]
#[result_type(())]
pub struct FileChanged {
    pub rel_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn storage_write_msg_fields() {
        let msg = StorageWrite {
            path: "alice/agent1/2026-03-31.md".to_string(),
            content: "hello".to_string(),
        };
        assert_eq!(msg.path, "alice/agent1/2026-03-31.md");
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn file_op_write_msg_fields() {
        let msg = FileOpWrite {
            username: "alice".to_string(),
            agent_id: Uuid::new_v4(),
            filename: "2026-03-31.md".to_string(),
            content: "hello".to_string(),
        };
        assert_eq!(msg.username, "alice");
    }

    #[test]
    fn search_result_fields() {
        let sr = SearchResult {
            path: "alice/agent1/2026-03-31.md".to_string(),
            start_line: 1,
            end_line: 10,
            score: 0.95,
            snippet: "hello world".to_string(),
        };
        assert!(sr.score > 0.9);
    }

    #[test]
    fn file_changed_msg_fields() {
        let msg = FileChanged {
            rel_path: "alice/agent1/x.md".to_string(),
        };
        assert_eq!(msg.rel_path, "alice/agent1/x.md");
    }

    #[test]
    fn chunk_fields() {
        use crate::llm::Embedding;
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
