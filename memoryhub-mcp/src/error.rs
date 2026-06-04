//! Error types for the MemoryHub MCP server.

use thiserror::Error;

/// Configuration errors surfaced at startup.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("MEMORYHUB_URL is required")]
    MissingUrl,

    #[error("MEMORYHUB_TOKEN is required")]
    MissingToken,

    #[error("MEMORYHUB_AGENT_ID is not a valid UUID: {0}")]
    BadAgentId(String),
}

/// Errors from a MemoryHub API call.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("authentication failed — check MEMORYHUB_TOKEN")]
    Unauthorized,

    #[error("cannot reach MemoryHub at {0}")]
    Unreachable(String),

    #[error("MemoryHub returned {status}: {body}")]
    Http { status: u16, body: String },

    #[error("unexpected response from MemoryHub: {0}")]
    Decode(String),
}

/// Errors specific to `upload_memory`'s filesystem handling.
#[derive(Debug, Error)]
pub enum UploadError {
    #[error("path must be absolute")]
    NotAbsolute,

    #[error("cannot read {0}")]
    Io(String),

    #[error(transparent)]
    Client(#[from] ClientError),
}
