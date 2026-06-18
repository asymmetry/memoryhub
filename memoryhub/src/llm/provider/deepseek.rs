//! DeepSeek provider — OpenAI-compatible HTTP API.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use acktor::ErrorReport;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::super::{config::LlmConfig, error::LlmError};
use super::{ChatMessage, ChatResponse, Provider, Role, map_reqwest, map_status};

pub struct DeepSeekProvider {
    http: Client,
    base_url: String,
    api_key: String,
    chat_model: String,
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
            .map_err(|e| {
                LlmError::Provider(format!("reqwest client build failed: {}", e.report()))
            })?;

        Ok(Self {
            http,
            base_url: config.base_url.clone(),
            api_key,
            chat_model: config.model.clone(),
        })
    }
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

impl Provider for DeepSeekProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/chat/completions", self.base_url);

            debug!(
                "Sending chat with {} messages to DeepSeek at {}",
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    fn config_for(server: &MockServer) -> LlmConfig {
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "test-key") };
        LlmConfig {
            provider: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            model: "deepseek-v4-flash".into(),
            synthesis_idle_timeout_secs: 60,
            max_retries: 3,
            request_timeout_secs: 5,
            base_url: server.uri(),
            ..LlmConfig::default()
        }
    }

    #[tokio::test]
    async fn chat_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "deepseek-v4-flash",
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

        assert_eq!(out.model, "deepseek-v4-flash");
        assert_eq!(out.content, "hello back");
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
