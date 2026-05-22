//! OpenAI provider — chat completions and embeddings.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use acktor::ErrorReport;
use acktor::utils::debug_trace;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::llm::config::LlmConfig;
use crate::llm::provider::{ChatMessage, ChatResponse, EmbeddingProvider, Provider, Role};
use crate::llm::{EmbedResult, Embedding, LlmError};

pub struct OpenAiProvider {
    http: Client,
    base_url: String,
    api_key: String,
    chat_model: String,
    embedding_model: String,
}

impl OpenAiProvider {
    /// Construct for the **chat** role: reads `api_key_env` and `base_url`.
    /// Used when OpenAI is the chat provider, and when it serves both roles
    /// (in which case it is treated as a chat provider).
    pub fn new_chat(config: &LlmConfig) -> Result<Self, LlmError> {
        Self::from_parts(config, &config.api_key_env, &config.base_url)
    }

    /// Construct for the **embedding** role: reads `embedding_api_key_env` and
    /// `embedding_base_url`. Used when OpenAI is the embedding provider while a
    /// different vendor (e.g. DeepSeek) handles chat.
    pub fn new_embedding(config: &LlmConfig) -> Result<Self, LlmError> {
        Self::from_parts(
            config,
            &config.embedding_api_key_env,
            &config.embedding_base_url,
        )
    }

    fn from_parts(config: &LlmConfig, api_key_env: &str, base_url: &str) -> Result<Self, LlmError> {
        let api_key = std::env::var(api_key_env).map_err(|_| {
            LlmError::Config(format!("environment variable {} is not set", api_key_env))
        })?;
        let http = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|e| {
                LlmError::Provider(format!("reqwest client build failed: {}", e.report()))
            })?;

        Ok(Self {
            http,
            base_url: base_url.to_string(),
            api_key,
            chat_model: config.model.clone(),
            embedding_model: config.embedding_model.clone(),
        })
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponseData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    model: String,
    data: Vec<EmbedResponseData>,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatRequestMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatRequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponseRaw {
    model: String,
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

async fn map_status(resp: reqwest::Response) -> Result<reqwest::Response, LlmError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let excerpt = if body.len() > 512 {
        &body[..512]
    } else {
        &body
    };
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        Err(LlmError::Transient(format!("{}: {}", status, excerpt)))
    } else {
        Err(LlmError::Provider(format!("{}: {}", status, excerpt)))
    }
}

fn map_reqwest(e: reqwest::Error) -> LlmError {
    if e.is_timeout() || e.is_connect() {
        LlmError::Transient(e.to_string())
    } else {
        LlmError::Provider(e.to_string())
    }
}

impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/chat/completions", self.base_url);
            debug_trace!(
                "Sending chat with {} messages to OpenAI at {}",
                messages.len(),
                url
            );

            let body = ChatRequest {
                model: &self.chat_model,
                messages: messages
                    .iter()
                    .map(|m| ChatRequestMessage {
                        role: role_str(m.role),
                        content: &m.content,
                    })
                    .collect(),
                stream: false,
            };

            let resp = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(map_reqwest)?;
            let resp = map_status(resp).await?;

            let parsed: ChatResponseRaw = resp.json().await.map_err(map_reqwest)?;
            let content = parsed
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LlmError::Provider("chat response had no choices".into()))?
                .message
                .content;

            Ok(ChatResponse {
                model: parsed.model,
                content,
            })
        })
    }
}

impl EmbeddingProvider for OpenAiProvider {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/embeddings", self.base_url);
            debug_trace!("Embedding {} texts via OpenAI at {}", texts.len(), url);

            let resp = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&EmbedRequest {
                    model: &self.embedding_model,
                    input: texts,
                })
                .send()
                .await
                .map_err(map_reqwest)?;
            let resp = map_status(resp).await?;
            let parsed: EmbedResponse = resp.json().await.map_err(map_reqwest)?;

            Ok(EmbedResult {
                model: parsed.model,
                embeddings: parsed
                    .data
                    .into_iter()
                    .map(|d| Embedding(d.embedding))
                    .collect(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    fn config_for(server: &MockServer) -> LlmConfig {
        unsafe { std::env::set_var("OPENAI_API_KEY", "test-key") };
        LlmConfig {
            provider: "openai".into(),
            embedding_provider: "openai".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            embedding_api_key_env: "OPENAI_API_KEY".into(),
            model: "gpt-4o-mini".into(),
            embedding_model: "text-embedding-3-small".into(),
            embedding_dim: Some(3),
            request_timeout_secs: 5,
            base_url: server.uri(),
            embedding_base_url: server.uri(),
            ..LlmConfig::default()
        }
    }

    #[tokio::test]
    async fn embed_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "text-embedding-3-small",
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] },
                    { "embedding": [0.4, 0.5, 0.6] }
                ]
            })))
            .mount(&server)
            .await;

        let p = OpenAiProvider::new_embedding(&config_for(&server)).unwrap();
        let out = p.embed(&["a".to_string(), "b".to_string()]).await.unwrap();

        assert_eq!(out.model, "text-embedding-3-small");
        assert_eq!(out.embeddings.len(), 2);
        assert_eq!(out.embeddings[0].0, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn chat_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "gpt-4o-mini",
                "choices": [{ "message": { "content": "hello back" } }]
            })))
            .mount(&server)
            .await;

        let p = OpenAiProvider::new_chat(&config_for(&server)).unwrap();
        let out = p
            .chat(&[ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }])
            .await
            .unwrap();

        assert_eq!(out.model, "gpt-4o-mini");
        assert_eq!(out.content, "hello back");
    }

    #[tokio::test]
    async fn missing_env_var_returns_config_error() {
        unsafe { std::env::remove_var("OPENAI_API_KEY_MISSING") };
        let cfg = LlmConfig {
            api_key_env: "OPENAI_API_KEY_MISSING".into(),
            ..LlmConfig::default()
        };
        let err = OpenAiProvider::new_chat(&cfg).map(|_| ()).unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
    }

    /// The chat role reads `api_key_env` / `base_url`; the embedding role reads
    /// `embedding_api_key_env` / `embedding_base_url`. The two must not be crossed.
    #[tokio::test]
    async fn roles_read_their_own_env_and_base_url() {
        unsafe {
            std::env::set_var("CHAT_KEY", "chat-key");
            std::env::set_var("EMBED_KEY", "embed-key");
        }
        let cfg = LlmConfig {
            api_key_env: "CHAT_KEY".into(),
            embedding_api_key_env: "EMBED_KEY".into(),
            base_url: "https://chat.example".into(),
            embedding_base_url: "https://embed.example".into(),
            ..LlmConfig::default()
        };

        let chat = OpenAiProvider::new_chat(&cfg).unwrap();
        assert_eq!(chat.api_key, "chat-key");
        assert_eq!(chat.base_url, "https://chat.example");

        let embed = OpenAiProvider::new_embedding(&cfg).unwrap();
        assert_eq!(embed.api_key, "embed-key");
        assert_eq!(embed.base_url, "https://embed.example");
    }
}
