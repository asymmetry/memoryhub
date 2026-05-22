//! Error types for the LLM Service sub-system.

use acktor::error::{BoxError, RecvError, SendError};
use thiserror::Error;

/// Errors from LLM operations.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("LLM config error: {0}")]
    Config(String),

    #[error("unknown LLM provider: {0}")]
    UnknownProvider(String),

    /// Non-retryable provider failure: 4xx, parse error, auth error.
    #[error("LLM provider error: {0}")]
    Provider(String),

    #[error("could not load prompt template")]
    LoadTemplate(#[from] std::io::Error),

    #[error("could not write default prompt templates")]
    WriteDefaultTemplates(std::io::Error),

    /// Retryable failure: network timeout, HTTP 5xx, HTTP 429.
    #[error("transient LLM error: {0}")]
    Transient(String),

    #[error("could not send message")]
    SendError(#[source] BoxError),

    #[error("could not receive message")]
    RecvError(#[from] RecvError),
}

impl<M> From<SendError<M>> for LlmError
where
    M: Send + Sync + 'static,
{
    fn from(e: SendError<M>) -> Self {
        Self::SendError(e.into())
    }
}
