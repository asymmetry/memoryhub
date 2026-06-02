//! Error types for the memory sub-system.

use std::path::PathBuf;

use acktor::error::{BoxError, RecvError, SendError};
use thiserror::Error;

/// Errors from filesystem storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O error at {}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Errors from index operations.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("embedding dimension mismatch: stored={stored}, received={received}")]
    DimensionMismatch { stored: usize, received: usize },
}

/// Top-level error for memory operations.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Indexer(#[from] IndexError),

    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),

    #[error("invalid project: {0}")]
    InvalidProject(String),

    #[error("could not send message")]
    SendError(#[source] BoxError),

    #[error("could not receive message")]
    RecvError(#[from] RecvError),
}

impl<M> From<SendError<M>> for MemoryError
where
    M: Send + Sync + 'static,
{
    fn from(e: SendError<M>) -> Self {
        Self::SendError(e.into())
    }
}
