//! DeepSeek provider — OpenAI-compatible HTTP API.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::trace;

use crate::llm::config::LlmConfig;
use crate::llm::provider::{ChatMessage, ChatResponse, Provider, Role};
use crate::llm::{EmbedResult, Embedding, LlmError};

pub struct DeepSeekProvider {
    http: Client,
    base_url: String,
    api_key: String,
    chat_model: String,
    embedding_model: String,
}

impl DeepSeekProvider {
    pub fn new(config: &LlmConfig) -> Result<Self, LlmError> {
        let api_key = std::env::var(&config.api_key_env).map_err(|_| {
            LlmError::Config(format!(
                "environment variable {} is not set",
                config.api_key_env
            ))
        })?;
        let http = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|e| LlmError::Config(format!("reqwest client build failed: {}", e)))?;
        Ok(Self {
            http,
            base_url: config.base_url.clone(),
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

impl Provider for DeepSeekProvider {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/v1/embeddings", self.base_url);
            trace!(url, n = texts.len(), "deepseek embed");
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

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/v1/chat/completions", self.base_url);
            trace!(url, n = messages.len(), "deepseek chat");
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn config_for(server: &MockServer) -> LlmConfig {
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "test-key") };
        LlmConfig {
            provider: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            model: "deepseek-chat".into(),
            embedding_model: "deepseek-embedding".into(),
            embedding_dim: Some(3),
            session_idle_timeout_secs: 60,
            max_retries: 3,
            request_timeout_secs: 5,
            base_url: server.uri(),
        }
    }

    #[tokio::test]
    async fn embed_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "deepseek-embedding",
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] },
                    { "embedding": [0.4, 0.5, 0.6] }
                ]
            })))
            .mount(&server)
            .await;

        let p = DeepSeekProvider::new(&config_for(&server)).unwrap();
        let out = p.embed(&["a".to_string(), "b".to_string()]).await.unwrap();

        assert_eq!(out.model, "deepseek-embedding");
        assert_eq!(out.embeddings.len(), 2);
        assert_eq!(out.embeddings[0].0, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn chat_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "deepseek-chat",
                "choices": [{ "message": { "content": "hello back" } }]
            })))
            .mount(&server)
            .await;

        let p = DeepSeekProvider::new(&config_for(&server)).unwrap();
        let out = p
            .chat(&[ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }])
            .await
            .unwrap();

        assert_eq!(out.model, "deepseek-chat");
        assert_eq!(out.content, "hello back");
    }

    #[tokio::test]
    async fn http_429_maps_to_transient() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
            .mount(&server)
            .await;

        let p = DeepSeekProvider::new(&config_for(&server)).unwrap();
        let err = p.embed(&["x".to_string()]).await.unwrap_err();
        assert!(matches!(err, LlmError::Transient(_)));
    }

    #[tokio::test]
    async fn http_400_maps_to_provider() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad input"))
            .mount(&server)
            .await;

        let p = DeepSeekProvider::new(&config_for(&server)).unwrap();
        let err = p.embed(&["x".to_string()]).await.unwrap_err();
        assert!(matches!(err, LlmError::Provider(_)));
    }

    #[tokio::test]
    async fn missing_env_var_returns_config_error() {
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY_MISSING") };
        let cfg = LlmConfig {
            api_key_env: "DEEPSEEK_API_KEY_MISSING".into(),
            ..LlmConfig::default()
        };
        let err = DeepSeekProvider::new(&cfg).map(|_| ()).unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
    }
}
