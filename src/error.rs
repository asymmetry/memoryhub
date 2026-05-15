//! Error types for ClawChorus.
//!
//! Module-specific errors live in their respective sub-modules.
//! This module defines top-level errors and re-exports sub-module errors.

use std::path::PathBuf;

use thiserror::Error;

// Re-export sub-system errors.
use crate::http::HttpServerError;
pub use crate::llm::LlmError;
pub use crate::memory::error::{IndexError, MemoryError, StorageError};

/// Errors from configuration loading.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config file {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("could not determine home directory")]
    NoHomeDir,
}

/// Top-level error for the [`crate::manager::Manager`] actor.
#[derive(Debug, Error)]
pub enum ManagerError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Http(#[from] HttpServerError),
    #[error("actor messaging error: {0}")]
    Actor(String),
}
