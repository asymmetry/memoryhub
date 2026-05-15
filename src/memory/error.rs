//! Error types for the Memory Manager sub-system.

use std::path::PathBuf;

use thiserror::Error;

/// Errors from filesystem storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Errors from index operations.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("index SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("index task join error: {0}")]
    TaskJoin(String),
    #[error("embedding dimension mismatch: stored={stored}, received={received}")]
    DimensionMismatch { stored: usize, received: usize },
}

/// Top-level error for Memory Manager operations.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),
    #[error("actor messaging error: {0}")]
    Actor(String),
}
