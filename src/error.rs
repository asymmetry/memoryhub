//! Error types for ClawChorus.
//!
//! Module-specific errors live in their respective sub-modules.
//! This module defines top-level errors and re-exports sub-module errors.

use std::path::PathBuf;

use thiserror::Error;

pub use crate::http::HttpServerError;
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

/// Top-level error for the [`crate::ClawChorus`] actor.
#[derive(Debug, Error)]
pub enum ClawChorusError {
    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Memory(#[from] MemoryError),

    #[error(transparent)]
    Http(#[from] HttpServerError),
}
