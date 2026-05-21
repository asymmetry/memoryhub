//! Provider trait + helpers for LLM Service.
//!
//! Each provider (DeepSeek, OpenAI, ...) is a `Provider` impl.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use tokio::time;
use tracing::warn;

use super::config::LlmConfig;
use super::{EmbedResult, LlmError};

pub mod deepseek;
pub mod openai;

#[doc(hidden)]
#[cfg(any(test, feature = "_test"))]
pub mod mock;

/// Role of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

/// A single chat turn.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// Response from a chat completion call.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub model: String,
    pub content: String,
}

/// Abstract chat provider. Implementations are plain async types (no actor).
pub trait Provider: Send + Sync + 'static {
    /// Short provider identifier, e.g. `"deepseek"`. Used to resolve provider-specific prompt
    /// templates.
    fn name(&self) -> &str;

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>>;
}

/// Abstract embedding provider. Kept separate from [`Provider`] because not every chat vendor
/// exposes an embeddings endpoint.
pub trait EmbeddingProvider: Send + Sync + 'static {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>>;
}

/// Builds chat and embedding providers from config.
///
/// When `provider == embedding_provider` and that provider supports both roles, a single
/// instance is shared between both Arcs to avoid duplicate HTTP clients and credential reads.
#[allow(clippy::type_complexity)]
pub fn build_providers(
    config: &LlmConfig,
) -> Result<(Arc<dyn Provider>, Arc<dyn EmbeddingProvider>), LlmError> {
    if config.provider == config.embedding_provider {
        match config.provider.as_str() {
            "openai" => {
                let p = Arc::new(openai::OpenAiProvider::new(config)?);
                return Ok((p.clone(), p));
            }
            #[cfg(any(test, feature = "_test"))]
            "mock" => {
                let p = Arc::new(mock::MockProvider::default());
                return Ok((p.clone(), p));
            }
            _ => {}
        }
    }

    let chat: Arc<dyn Provider> = match config.provider.as_str() {
        "deepseek" => Arc::new(deepseek::DeepSeekProvider::new(config)?),
        "openai" => Arc::new(openai::OpenAiProvider::new(config)?),
        #[cfg(any(test, feature = "_test"))]
        "mock" => Arc::new(mock::MockProvider::default()),
        other => return Err(LlmError::UnknownProvider(other.to_string())),
    };

    let embedding: Arc<dyn EmbeddingProvider> = match config.embedding_provider.as_str() {
        "openai" => Arc::new(openai::OpenAiProvider::new(config)?),
        #[cfg(any(test, feature = "_test"))]
        "mock" => Arc::new(mock::MockProvider::default()),
        other => return Err(LlmError::UnknownProvider(other.to_string())),
    };

    Ok((chat, embedding))
}

pub(crate) async fn retry<F, Fut, T>(max_attempts: u32, mut f: F) -> Result<T, LlmError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    let max_attempts = max_attempts.max(1);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match f().await {
            Ok(v) => return Ok(v),
            Err(LlmError::Transient(msg)) if attempt < max_attempts => {
                let base_ms = match attempt {
                    1 => 250u64,
                    2 => 500u64,
                    _ => 1000u64,
                };
                let jitter = rand::rng().random::<f64>();
                let delay = Duration::from_millis((base_ms as f64 * (0.5 + 0.5 * jitter)) as u64);
                warn!(
                    attempt,
                    "transient LLM error, retrying in {:?}: {}", delay, msg
                );
                time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockProvider;
    use super::*;

    #[tokio::test]
    async fn retry_succeeds_after_two_transient_errors() {
        let mock = MockProvider::new();
        // Stack-pop is LIFO. We want call order Transient, Transient, Ok, so push Ok first, then Transients on top.
        mock.push_chat(Ok(ChatResponse {
            model: "m".into(),
            content: "ok".into(),
        }));
        mock.push_chat(Err(LlmError::Transient("t2".into())));
        mock.push_chat(Err(LlmError::Transient("t1".into())));

        let msgs = vec![ChatMessage {
            role: Role::User,
            content: "hi".into(),
        }];
        let out = retry(3, || mock.chat(&msgs)).await.unwrap();

        assert_eq!(out.content, "ok");
        assert_eq!(mock.chat_call_count(), 3);
    }

    #[tokio::test]
    async fn retry_returns_provider_error_immediately() {
        let mock = MockProvider::new();
        mock.push_chat(Err(LlmError::Provider("boom".into())));

        let msgs = vec![ChatMessage {
            role: Role::User,
            content: "hi".into(),
        }];
        let err = retry(3, || mock.chat(&msgs)).await.unwrap_err();

        assert!(matches!(err, LlmError::Provider(_)));
        assert_eq!(mock.chat_call_count(), 1);
    }

    #[tokio::test]
    async fn retry_exhausts_returns_last_transient() {
        let mock = MockProvider::new();
        mock.push_chat(Err(LlmError::Transient("t3".into())));
        mock.push_chat(Err(LlmError::Transient("t2".into())));
        mock.push_chat(Err(LlmError::Transient("t1".into())));

        let msgs = vec![ChatMessage {
            role: Role::User,
            content: "hi".into(),
        }];
        let err = retry(3, || mock.chat(&msgs)).await.unwrap_err();

        assert!(matches!(err, LlmError::Transient(_)));
        assert_eq!(mock.chat_call_count(), 3);
    }

    #[test]
    fn build_providers_rejects_unknown_chat() {
        let cfg = LlmConfig {
            provider: "no-such".into(),
            ..LlmConfig::default()
        };
        let err = build_providers(&cfg).map(|_| ()).unwrap_err();
        assert!(matches!(err, LlmError::UnknownProvider(_)));
    }
}
