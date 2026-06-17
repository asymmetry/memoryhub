//! Message types for the [`MemoryManager`][super::MemoryManager] and its child actors.

use acktor::Message;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::llm::{Embedding, SynthesisTarget};
use crate::memory::error::{IndexError, MemoryError, StorageError};

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Identity scope for a search request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchScope {
    /// Whole store, across every user (plus the global summary).
    #[default]
    All,
    /// The caller's user: `{username}/%`.
    User,
    /// The caller's user+agent: `{username}/{agent_id}/%`.
    Agent,
}

/// Which synthesized summary tier to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryScope {
    User,
    Agent,
    Global,
}

impl SummaryScope {
    /// Resolve to a `SynthesisTarget`; `Agent` needs an `agent_id` (else `None`).
    pub fn target(self, username: String, agent_id: Option<Uuid>) -> Option<SynthesisTarget> {
        match self {
            SummaryScope::User => Some(SynthesisTarget::User { username }),
            SummaryScope::Global => Some(SynthesisTarget::Global),
            SummaryScope::Agent => agent_id.map(|id| SynthesisTarget::Agent {
                username,
                agent_id: id.to_string(),
            }),
        }
    }
}

/// A synthesized summary returned by `GetSummary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub content: String,
    pub path: String,
}

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

/// Search the index using embedding vectors.
#[derive(Debug, Message)]
#[result_type(Result<Vec<SearchResult>, IndexError>)]
pub struct IndexSearch {
    pub embeddings: Vec<Embedding>,
    pub username: String,
    pub agent_id: Uuid,
    pub scope: SearchScope,
    pub raw_only: bool,
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
    pub project: Option<String>,
    pub filename: String,
    pub content: String,
}

/// Read a memory file's content.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Option<String>, MemoryError>)]
pub struct FileOpRead {
    pub username: String,
    pub agent_id: Uuid,
    pub project: Option<String>,
    pub filename: String,
}

/// Delete a memory file (index + store).
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), MemoryError>)]
pub struct FileOpDelete {
    pub username: String,
    pub agent_id: Uuid,
    pub project: Option<String>,
    pub filename: String,
}

/// Search memories for a user+agent.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Vec<SearchResult>, MemoryError>)]
pub struct Search {
    pub username: String,
    pub agent_id: Uuid,
    pub scope: SearchScope,
    pub raw_only: bool,
    pub query: String,
}

/// Fetch the latest synthesized summary for a tier.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Option<Summary>, MemoryError>)]
pub struct GetSummary {
    pub username: String,
    pub agent_id: Option<Uuid>,
    pub scope: SummaryScope,
}

// ---------------------------------------------------------------------------
// Synthesizer messages
// ---------------------------------------------------------------------------

/// Fire-and-forget notification that a memory file was written or deleted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Message)]
#[result_type(())]
pub struct FileChanged {
    pub username: String,
    pub agent_id: Uuid,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn summary_scope_serde_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&SummaryScope::Global).unwrap(),
            "\"global\""
        );
        let s: SummaryScope = serde_json::from_str("\"agent\"").unwrap();
        assert_eq!(s, SummaryScope::Agent);
    }

    #[test]
    fn summary_scope_target_maps_tiers() {
        use crate::llm::SynthesisTarget;
        let uid = Uuid::nil();
        assert_eq!(
            SummaryScope::User.target("alice".into(), None),
            Some(SynthesisTarget::User {
                username: "alice".into()
            })
        );
        assert_eq!(
            SummaryScope::Global.target("alice".into(), None),
            Some(SynthesisTarget::Global)
        );
        assert_eq!(
            SummaryScope::Agent.target("alice".into(), Some(uid)),
            Some(SynthesisTarget::Agent {
                username: "alice".into(),
                agent_id: uid.to_string()
            })
        );
        // Agent scope without an agent_id yields nothing.
        assert_eq!(SummaryScope::Agent.target("alice".into(), None), None);
    }
}
