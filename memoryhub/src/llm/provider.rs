//! Provider trait + helpers for LLM Service.
//!
//! Each provider (DeepSeek, OpenAI, ...) is a `Provider` impl.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use reqwest::StatusCode;
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
    /// Returns a short provider identifier.
    ///
    /// Used to resolve provider-specific prompt templates.
    fn name(&self) -> &str;

    /// Sends `messages` to the provider and returns the response.
    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>>;
}

/// Abstract embedding provider. Kept separate from [`Provider`] because not every chat vendor
/// exposes an embeddings endpoint.
pub trait EmbeddingProvider: Send + Sync + 'static {
    /// Embeds `texts`, returning exactly one embedding per input in the **same order**.
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
            // A provider serving both roles is treated as a chat provider:
            // it reads `api_key_env` / `base_url`.
            "openai" => {
                let p = Arc::new(openai::OpenAiProvider::new_chat(config)?);
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
        "openai" => Arc::new(openai::OpenAiProvider::new_chat(config)?),
        #[cfg(any(test, feature = "_test"))]
        "mock" => Arc::new(mock::MockProvider::default()),
        other => return Err(LlmError::UnknownProvider(other.to_string())),
    };

    let embedding: Arc<dyn EmbeddingProvider> = match config.embedding_provider.as_str() {
        "openai" => Arc::new(openai::OpenAiProvider::new_embedding(config)?),
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
                    "transient LLM error on attempt {}, retrying in {} ms: {}",
                    attempt,
                    delay.as_millis(),
                    msg
                );
                time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Finds the closest x not exceeding index where is_char_boundary(x) is true.
///
/// FIXME: use [`str::floor_char_boundary`] once MSRV >= 1.91.
pub(crate) fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut boundary = index;
    while !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Maps a non-success HTTP response to an `LlmError`, classifying 5xx and 429 as `Transient`
/// (worth retrying) and everything else as `Provider`. The error body is truncated to a 512-char
/// boundary so a huge or multibyte body can't bloat the message or panic on a non-boundary slice.
///
/// Shared by every HTTP provider; the OpenAI-compatible chat/embedding APIs use the same scheme.
pub(crate) async fn map_status(resp: reqwest::Response) -> Result<reqwest::Response, LlmError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let excerpt = &body[..floor_char_boundary(&body, 512)];
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        Err(LlmError::Transient(format!("{}: {}", status, excerpt)))
    } else {
        Err(LlmError::Provider(format!("{}: {}", status, excerpt)))
    }
}

/// Classifies a reqwest transport error: timeouts and connection failures are `Transient`.
pub(crate) fn map_reqwest(e: reqwest::Error) -> LlmError {
    if e.is_timeout() || e.is_connect() {
        LlmError::Transient(e.to_string())
    } else {
        LlmError::Provider(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    use super::mock::MockProvider;
    use super::*;

    /// Fetches a real `reqwest::Response` with the given status and body via a one-shot mock
    /// server, so `map_status` can be exercised without a live provider.
    async fn fetched(status: u16, body: &str) -> reqwest::Response {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        reqwest::Client::new()
            .get(server.uri())
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn map_status_classifies_by_code() {
        // 2xx passes through; 5xx and 429 are transient (retried); other 4xx are permanent.
        assert!(map_status(fetched(200, "ok").await).await.is_ok());
        assert!(matches!(
            map_status(fetched(429, "slow down").await).await,
            Err(LlmError::Transient(_))
        ));
        assert!(matches!(
            map_status(fetched(503, "down").await).await,
            Err(LlmError::Transient(_))
        ));
        assert!(matches!(
            map_status(fetched(400, "bad input").await).await,
            Err(LlmError::Provider(_))
        ));
    }

    #[tokio::test]
    async fn map_status_truncates_long_multibyte_body_without_panicking() {
        // 3-byte chars, longer than the 512-char excerpt: a byte slice at index 512 would split a
        // codepoint and panic. Char-boundary truncation must not.
        let body = "界".repeat(600);
        let err = map_status(fetched(500, &body).await).await.unwrap_err();
        assert!(matches!(err, LlmError::Transient(_)));
    }

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
    fn floor_char_boundary_clamps_and_never_splits() {
        // 3-byte chars: only byte indices 0, 3, 6, 9 are char boundaries.
        let s = "界界界"; // 9 bytes
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 2), 0);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 5), 3);
        assert_eq!(floor_char_boundary(s, 6), 6);
        // Past the end clamps to len.
        assert_eq!(floor_char_boundary(s, 100), s.len());
        // The result is always a valid slice index.
        assert_eq!(&s[..floor_char_boundary(s, 5)], "界");
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
