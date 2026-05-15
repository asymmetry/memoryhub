//! Error types for the LLM Service sub-system.

use thiserror::Error;

/// Errors from LLM operations.
#[derive(Debug, Error)]
pub enum LlmError {
    /// Retryable failure: network timeout, HTTP 5xx, HTTP 429.
    #[error("transient LLM error: {0}")]
    Transient(String),
    /// Non-retryable provider failure: 4xx, parse error, auth error.
    #[error("LLM provider error: {0}")]
    Provider(String),
    /// `LlmConfig::provider` did not match any known provider name.
    #[error("unknown LLM provider: {0}")]
    UnknownProvider(String),
    /// Actor framework messaging failure.
    #[error("LLM actor messaging error: {0}")]
    Actor(String),
    /// Configuration error (e.g. missing API key environment variable).
    #[error("LLM config error: {0}")]
    Config(String),
}
