# LLM Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the stub LLM Service with a real implementation backed by a `Provider` trait, a working DeepSeek provider, conversation history in `Session`, retries on transient failures, and acktor-cron-driven idle timeout.

**Architecture:** `LlmService` holds an `Arc<dyn Provider>` built once at startup and clones it into in-flight `Embed` futures and spawned `Session` actors. `Session` owns the full chat transcript and is a `CronActor` whose periodic `task()` self-terminates on idle. Provider impls are plain async — no `async_trait` crate.

**Tech Stack:** Rust 2024, acktor 1.1 (with `CronActor` / `CronContext`), reqwest, tokio, thiserror, rand (for jitter), wiremock (dev-dep).

Spec: `docs/superpowers/specs/llm-service-design.md`.

---

## File Structure

**Create:**
- `src/llm/provider.rs` — `Provider` trait, `ChatMessage`, `ChatResponse`, `Role`, `retry` helper, `build_provider`, `MockProvider` (cfg-test).
- `src/llm/provider/deepseek.rs` — `DeepSeekProvider` impl + wiremock-based tests.

**Modify:**
- `src/llm.rs` — `LlmService` gains `Arc<dyn Provider>`; `Embed` handler uses retry; `StartSession` passes provider + model + retries.
- `src/llm/session.rs` — `CronContext`, history field, retry-wrapped `chat`, cron-based idle timeout.
- `src/llm/error.rs` — add `Transient`, `UnknownProvider`, `Config` variants.
- `src/llm/config.rs` — add `max_retries`, `request_timeout_secs`, `base_url`.
- `src/main.rs` — call `build_provider` and pass `Arc<dyn Provider>` into `LlmService::new`.
- `Cargo.toml` — add `rand` dep, `wiremock` dev-dep.
- `docs/superpowers/specs/clawchorus-design.md` — parent-spec correction (drop "spawns short-lived child per request").
- `docs/superpowers/specs/memory-manager-design.md` — drop the same parenthetical from the Embed row.

---

## Task 1: Cargo dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add `rand` to `[dependencies]` and `wiremock` to `[dev-dependencies]`**

Final lines (insert alphabetically):

```toml
rand = "0.9"
```

```toml
[dev-dependencies]
tempfile = "3"
wiremock = "0.6"
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add rand and wiremock for LLM service"
```

---

## Task 2: Extend `LlmError`

**Files:**
- Modify: `src/llm/error.rs`

- [ ] **Step 1: Replace the file contents**

```rust
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
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: success (no callers reference the dropped variants).

- [ ] **Step 3: `cargo fmt`**

Run: `cargo fmt`

- [ ] **Step 4: Commit**

```bash
git add src/llm/error.rs
git commit -m "feat(llm): extend LlmError with transient/config/unknown-provider"
```

---

## Task 3: Extend `LlmConfig`

**Files:**
- Modify: `src/llm/config.rs`

- [ ] **Step 1: Add three new fields plus their default fns**

Replace the file with:

```rust
use serde::{Deserialize, Serialize};

/// LLM / embedding provider settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider identifier, e.g. `"deepseek"`.
    pub provider: String,
    /// Name of the environment variable that holds the API key.
    pub api_key_env: String,
    /// Chat / completion model identifier.
    pub model: String,
    /// Embedding model identifier.
    pub embedding_model: String,
    /// Embedding vector dimension. `None` means auto-detect from the first
    /// embedding response. Pin it for known models to avoid surprises.
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    /// Seconds a `Session` will sit idle before self-terminating.
    #[serde(default = "default_session_idle_timeout_secs")]
    pub session_idle_timeout_secs: u64,
    /// Maximum number of attempts (including the first) for a single
    /// provider call on transient failure.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// HTTP request timeout in seconds for provider calls.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Provider base URL (override for testing / self-hosted gateways).
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

fn default_session_idle_timeout_secs() -> u64 { 600 }
fn default_max_retries() -> u32 { 3 }
fn default_request_timeout_secs() -> u64 { 30 }
fn default_base_url() -> String { "https://api.deepseek.com".to_string() }

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            model: "deepseek-chat".to_string(),
            embedding_model: "deepseek-embedding".to_string(),
            embedding_dim: None,
            session_idle_timeout_secs: 600,
            max_retries: 3,
            request_timeout_secs: 30,
            base_url: "https://api.deepseek.com".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_populated() {
        let c = LlmConfig::default();
        assert_eq!(c.session_idle_timeout_secs, 600);
        assert_eq!(c.max_retries, 3);
        assert_eq!(c.request_timeout_secs, 30);
        assert_eq!(c.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn partial_toml_uses_defaults_for_new_fields() {
        let toml_in = r#"
provider = "deepseek"
api_key_env = "DEEPSEEK_API_KEY"
model = "deepseek-chat"
embedding_model = "deepseek-embedding"
"#;
        let c: LlmConfig = toml::from_str(toml_in).unwrap();
        assert_eq!(c.max_retries, 3);
        assert_eq!(c.request_timeout_secs, 30);
        assert_eq!(c.base_url, "https://api.deepseek.com");
    }
}
```

- [ ] **Step 2: Run config tests**

Run: `cargo test --lib llm::config`
Expected: all pass.

- [ ] **Step 3: `cargo fmt`**

- [ ] **Step 4: Commit**

```bash
git add src/llm/config.rs
git commit -m "feat(llm): add max_retries, request_timeout, base_url to LlmConfig"
```

---

## Task 4: `Provider` trait, message types, and retry helper

**Files:**
- Create: `src/llm/provider.rs`
- Modify: `src/llm.rs` (add `pub mod provider;` and re-exports)

- [ ] **Step 1: Create `src/llm/provider.rs`**

```rust
//! Provider trait + helpers for LLM Service.
//!
//! Each provider (DeepSeek, OpenAI, ...) is a `Provider` impl. Providers are
//! plain async types, not actors — `LlmService` owns one `Arc<dyn Provider>`
//! at startup and clones it into in-flight futures and spawned sessions.

pub mod deepseek;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use tokio::time::sleep;
use tracing::warn;

use crate::llm::config::LlmConfig;
use crate::llm::{EmbedResult, LlmError};

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

/// Abstract LLM provider. Implementations are plain async types (no actor).
pub trait Provider: Send + Sync + 'static {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>>;

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>>;
}

/// Build a provider from config. Selects the impl by `config.provider`.
pub fn build_provider(config: &LlmConfig) -> Result<Arc<dyn Provider>, LlmError> {
    match config.provider.as_str() {
        "deepseek" => Ok(Arc::new(deepseek::DeepSeekProvider::new(config)?)),
        other => Err(LlmError::UnknownProvider(other.to_string())),
    }
}

/// Retry `f` up to `max_attempts` total. Retries only on `LlmError::Transient`.
/// Backoff: 250ms, 500ms, 1s — capped at 1s with full jitter.
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
                let delay = Duration::from_millis((base_ms as f64 * jitter) as u64);
                warn!(attempt, "transient LLM error, retrying in {:?}: {}", delay, msg);
                sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// MockProvider — used by tests in this crate.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod mock {
    use std::sync::Mutex;

    use crate::llm::Embedding;

    use super::*;

    /// A `Provider` that records calls and replays scripted responses.
    pub struct MockProvider {
        pub embed_replies: Mutex<Vec<Result<EmbedResult, LlmError>>>,
        pub chat_replies: Mutex<Vec<Result<ChatResponse, LlmError>>>,
        pub embed_calls: Mutex<Vec<Vec<String>>>,
        pub chat_calls: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl MockProvider {
        pub fn new() -> Self {
            Self {
                embed_replies: Mutex::new(Vec::new()),
                chat_replies: Mutex::new(Vec::new()),
                embed_calls: Mutex::new(Vec::new()),
                chat_calls: Mutex::new(Vec::new()),
            }
        }

        pub fn push_embed(&self, reply: Result<EmbedResult, LlmError>) {
            self.embed_replies.lock().unwrap().push(reply);
        }

        pub fn push_chat(&self, reply: Result<ChatResponse, LlmError>) {
            self.chat_replies.lock().unwrap().push(reply);
        }

        pub fn embed_call_count(&self) -> usize {
            self.embed_calls.lock().unwrap().len()
        }

        pub fn chat_call_count(&self) -> usize {
            self.chat_calls.lock().unwrap().len()
        }

        pub fn last_chat_call(&self) -> Option<Vec<ChatMessage>> {
            self.chat_calls.lock().unwrap().last().cloned()
        }

        pub fn canned_embed(dim: usize, count: usize, model: &str) -> EmbedResult {
            EmbedResult {
                model: model.to_string(),
                embeddings: (0..count).map(|_| Embedding(vec![0.1; dim])).collect(),
            }
        }
    }

    impl Provider for MockProvider {
        fn embed<'a>(
            &'a self,
            texts: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>> {
            self.embed_calls.lock().unwrap().push(texts.to_vec());
            let reply = self
                .embed_replies
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err(LlmError::Provider("no canned embed reply".into())));
            Box::pin(async move { reply })
        }

        fn chat<'a>(
            &'a self,
            messages: &'a [ChatMessage],
        ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>> {
            self.chat_calls.lock().unwrap().push(messages.to_vec());
            let reply = self
                .chat_replies
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err(LlmError::Provider("no canned chat reply".into())));
            Box::pin(async move { reply })
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
        // pushed in reverse: pop yields Ok first if we want Ok last → push Ok last (top of stack)
        // We want order: Transient, Transient, Ok. Stack pop is LIFO, so push Ok, Transient, Transient.
        mock.push_chat(Ok(ChatResponse { model: "m".into(), content: "ok".into() }));
        mock.push_chat(Err(LlmError::Transient("t2".into())));
        mock.push_chat(Err(LlmError::Transient("t1".into())));

        let msgs = vec![ChatMessage { role: Role::User, content: "hi".into() }];
        let out = retry(3, || mock.chat(&msgs)).await.unwrap();

        assert_eq!(out.content, "ok");
        assert_eq!(mock.chat_call_count(), 3);
    }

    #[tokio::test]
    async fn retry_returns_provider_error_immediately() {
        let mock = MockProvider::new();
        mock.push_chat(Err(LlmError::Provider("boom".into())));

        let msgs = vec![ChatMessage { role: Role::User, content: "hi".into() }];
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

        let msgs = vec![ChatMessage { role: Role::User, content: "hi".into() }];
        let err = retry(3, || mock.chat(&msgs)).await.unwrap_err();

        assert!(matches!(err, LlmError::Transient(_)));
        assert_eq!(mock.chat_call_count(), 3);
    }

    #[test]
    fn build_provider_rejects_unknown() {
        let mut cfg = LlmConfig::default();
        cfg.provider = "no-such".into();
        // we cannot actually construct DeepSeek without the env var, so test
        // only the unknown branch here.
        let err = build_provider(&cfg).unwrap_err();
        assert!(matches!(err, LlmError::UnknownProvider(_)));
    }
}
```

Note: `deepseek` is referenced in `build_provider` and as a submodule — task 5 creates it. Until then this file won't compile, so task 5 is the next step.

- [ ] **Step 2: Add `pub mod provider;` to `src/llm.rs`**

Add immediately after the existing `pub mod session;` line:

```rust
pub mod provider;
```

- [ ] **Step 3: Do NOT build yet** — the `deepseek` submodule comes next.

- [ ] **Step 4: Stage the changes (commit after task 5 passes)**

(Do not commit yet — wait until DeepSeek lands.)

---

## Task 5: DeepSeek provider implementation

**Files:**
- Create: `src/llm/provider/deepseek.rs`

- [ ] **Step 1: Write the file**

```rust
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

// ---------------------------------------------------------------------------
// Wire formats
// ---------------------------------------------------------------------------

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
    let excerpt = if body.len() > 512 { &body[..512] } else { &body };
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn config_for(server: &MockServer) -> LlmConfig {
        // SAFETY: tests are single-threaded per cfg unless run with --test-threads, but
        // each test sets its own var key. Use a stable key shared by all tests here.
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
        let out = p
            .embed(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();

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
            .chat(&[ChatMessage { role: Role::User, content: "hi".into() }])
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
        let mut cfg = LlmConfig::default();
        cfg.api_key_env = "DEEPSEEK_API_KEY_MISSING".into();
        let err = DeepSeekProvider::new(&cfg).unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
    }
}
```

- [ ] **Step 2: Build the crate**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Run provider tests**

Run: `cargo test --lib llm::provider`
Expected: all pass (including the deepseek wiremock tests and the retry tests from task 4).

- [ ] **Step 4: `cargo fmt`**

- [ ] **Step 5: Commit (covers tasks 4 and 5)**

```bash
git add src/llm.rs src/llm/provider.rs src/llm/provider/deepseek.rs
git commit -m "feat(llm): provider trait, retry helper, DeepSeek implementation"
```

---

## Task 6: Refactor `LlmService` to hold `Arc<dyn Provider>`

**Files:**
- Modify: `src/llm.rs`

- [ ] **Step 1: Replace the file contents**

```rust
//! LLM Service Actor — handles embedding and conversation sessions.
//!
//! Sibling of Memory Manager, supervised by the top-level Manager.

pub mod config;
pub mod error;
pub mod provider;
pub mod session;

use std::sync::Arc;
use std::time::Duration;

use acktor::message::FutureMessageResult;
use acktor::{Actor, Address, Context, Handler, Message};
use tracing::trace;

use crate::llm::config::LlmConfig;
pub use crate::llm::error::LlmError;
use crate::llm::provider::Provider;
use crate::llm::session::Session;

/// A single embedding vector.
#[derive(Debug, Clone)]
pub struct Embedding(pub Vec<f32>);

/// Result of an embedding request.
#[derive(Debug, Clone)]
pub struct EmbedResult {
    pub model: String,
    pub embeddings: Vec<Embedding>,
}

/// Embed a batch of text strings, returning one [`Embedding`] per input.
#[derive(Debug, Clone, Message)]
#[result_type(Result<EmbedResult, LlmError>)]
pub struct Embed {
    pub texts: Vec<String>,
}

/// Open a new conversation session. Reply is the spawned `Session` actor's address.
#[derive(Debug, Clone, Message)]
#[result_type(Result<Address<Session>, LlmError>)]
pub struct StartSession;

pub struct LlmService {
    config: LlmConfig,
    provider: Arc<dyn Provider>,
}

impl LlmService {
    pub fn new(config: LlmConfig, provider: Arc<dyn Provider>) -> Self {
        Self { config, provider }
    }
}

impl Actor for LlmService {
    type Context = Context<Self>;
    type Error = LlmError;
}

impl Handler<Embed> for LlmService {
    type Result = FutureMessageResult<Embed>;

    async fn handle(
        &mut self,
        msg: Embed,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<Embed> {
        trace!("Handle command {:?}", msg);
        let provider = self.provider.clone();
        let max_retries = self.config.max_retries;
        FutureMessageResult::new(async move {
            provider::retry(max_retries, || provider.embed(&msg.texts)).await
        })
    }
}

impl Handler<StartSession> for LlmService {
    type Result = FutureMessageResult<StartSession>;

    async fn handle(
        &mut self,
        msg: StartSession,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<StartSession> {
        trace!("Handle command {:?}", msg);
        let provider = self.provider.clone();
        let model = self.config.model.clone();
        let idle = Duration::from_secs(self.config.session_idle_timeout_secs);
        let max_retries = self.config.max_retries;
        FutureMessageResult::new(async move {
            let (addr, _handle) = Session::new(provider, model, idle, max_retries)
                .start("session")
                .map_err(|e| LlmError::Actor(e.to_string()))?;
            Ok(addr)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::mock::MockProvider;
    use crate::llm::provider::{ChatMessage, ChatResponse, Role};

    fn cfg() -> LlmConfig {
        let mut c = LlmConfig::default();
        c.session_idle_timeout_secs = 60;
        c
    }

    #[tokio::test]
    async fn embed_returns_mock_vectors() {
        let mock = Arc::new(MockProvider::new());
        mock.push_embed(Ok(MockProvider::canned_embed(3, 2, "mock-emb")));

        let svc = LlmService::new(cfg(), mock.clone());
        let (addr, _h) = svc.start("llm-test").unwrap();

        let out = addr
            .send(Embed {
                texts: vec!["a".into(), "b".into()],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(out.embeddings.len(), 2);
        assert_eq!(out.model, "mock-emb");
        assert_eq!(mock.embed_call_count(), 1);
    }

    #[tokio::test]
    async fn start_session_returns_working_address() {
        let mock = Arc::new(MockProvider::new());
        mock.push_chat(Ok(ChatResponse {
            model: "mock-chat".into(),
            content: "hello".into(),
        }));

        let svc = LlmService::new(cfg(), mock.clone());
        let (addr, _h) = svc.start("llm-test").unwrap();

        let sess = addr
            .send(StartSession)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let reply = sess
            .send(crate::llm::session::SendMessage {
                content: "hi".into(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(reply, "hello");
        let last = mock.last_chat_call().unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].role, Role::User);
        assert_eq!(last[0].content, "hi");
    }
}
```

- [ ] **Step 2: Do not build yet** — Session signature changes in task 7. Continue.

---

## Task 7: Convert `Session` to `CronContext` + history + retry

**Files:**
- Modify: `src/llm/session.rs`

- [ ] **Step 1: Replace the file contents**

```rust
//! Conversation session actor — child of `LlmService`, one per logical
//! conversation. Owns the full chat transcript and self-terminates on idle
//! via acktor's CronActor.

use std::sync::Arc;
use std::time::Duration;

use acktor::cron::{CronActor, CronContext};
use acktor::message::FutureMessageResult;
use acktor::{Actor, ActorContext, Handler, Message, Signal};
use tokio::time::Instant;
use tracing::{trace, warn};

use crate::llm::LlmError;
use crate::llm::provider::{ChatMessage, Provider, Role, retry};

/// Send a user-authored message into the session. Returns the assistant reply.
#[derive(Debug, Clone, Message)]
#[result_type(Result<String, LlmError>)]
pub struct SendMessage {
    pub content: String,
}

/// Gracefully stop the session.
#[derive(Debug, Clone, Message)]
#[result_type(Result<(), LlmError>)]
pub struct StopSession;

pub struct Session {
    provider: Arc<dyn Provider>,
    model: String,
    history: Vec<ChatMessage>,
    idle_timeout: Duration,
    last_activity: Instant,
    max_retries: u32,
}

impl Session {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: String,
        idle_timeout: Duration,
        max_retries: u32,
    ) -> Self {
        Self {
            provider,
            model,
            history: Vec::new(),
            idle_timeout,
            last_activity: Instant::now(),
            max_retries,
        }
    }

    /// Configured chat model (mainly for introspection / tests).
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl Actor for Session {
    type Context = CronContext<Self>;
    type Error = LlmError;
}

impl CronActor for Session {
    async fn task(&mut self, ctx: &mut Self::Context) -> Result<Duration, LlmError> {
        let elapsed = self.last_activity.elapsed();
        if elapsed >= self.idle_timeout {
            trace!("Session idle for {:?}, terminating", elapsed);
            let _ = ctx.address().do_send(Signal::Terminate).await;
            // Returned duration is effectively unused after Terminate is processed.
            return Ok(Duration::from_secs(3600));
        }
        let remaining = self.idle_timeout.saturating_sub(elapsed);
        Ok(remaining.max(Duration::from_secs(1)))
    }
}

impl Handler<SendMessage> for Session {
    type Result = FutureMessageResult<SendMessage>;

    async fn handle(
        &mut self,
        msg: SendMessage,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<SendMessage> {
        trace!("Handle command {:?}", msg);
        self.history.push(ChatMessage {
            role: Role::User,
            content: msg.content,
        });
        self.last_activity = Instant::now();

        // Clone what the future needs; we'll mutate `self.history` after.
        let provider = self.provider.clone();
        let max_retries = self.max_retries;
        let history_snapshot = self.history.clone();

        // We need to mutate `self.history` and `self.last_activity` AFTER the
        // call returns. Run the future inline (the actor's mailbox is held by
        // the wrapping FutureMessageResult; we capture self via &mut here is
        // not possible — so we run the call synchronously within handle and
        // return a ready future).
        let result = retry(max_retries, || provider.chat(&history_snapshot)).await;

        match result {
            Ok(resp) => {
                self.history.push(ChatMessage {
                    role: Role::Assistant,
                    content: resp.content.clone(),
                });
                self.last_activity = Instant::now();
                let content = resp.content;
                FutureMessageResult::new(async move { Ok(content) })
            }
            Err(e) => FutureMessageResult::new(async move { Err(e) }),
        }
    }
}

impl Handler<StopSession> for Session {
    type Result = FutureMessageResult<StopSession>;

    async fn handle(
        &mut self,
        msg: StopSession,
        ctx: &mut Self::Context,
    ) -> FutureMessageResult<StopSession> {
        trace!("Handle command {:?}", msg);
        let addr = ctx.address().clone();
        FutureMessageResult::new(async move {
            if let Err(e) = addr.do_send(Signal::Terminate).await {
                warn!("Session terminate failed: {}", e);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::ChatResponse;
    use crate::llm::provider::mock::MockProvider;

    fn mock() -> Arc<MockProvider> {
        Arc::new(MockProvider::new())
    }

    #[tokio::test]
    async fn send_message_appends_history_and_returns_reply() {
        let m = mock();
        m.push_chat(Ok(ChatResponse {
            model: "mock".into(),
            content: "hello back".into(),
        }));

        let session = Session::new(m.clone(), "mock-chat".into(), Duration::from_secs(60), 3);
        let (addr, _h) = session.start("sess-test").unwrap();

        let reply = addr
            .send(SendMessage { content: "hi".into() })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(reply, "hello back");
        let last = m.last_chat_call().unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].content, "hi");
    }

    #[tokio::test]
    async fn multi_turn_sends_full_history() {
        let m = mock();
        m.push_chat(Ok(ChatResponse { model: "mock".into(), content: "reply-2".into() }));
        m.push_chat(Ok(ChatResponse { model: "mock".into(), content: "reply-1".into() }));

        let session = Session::new(m.clone(), "mock-chat".into(), Duration::from_secs(60), 3);
        let (addr, _h) = session.start("sess-test").unwrap();

        let r1 = addr
            .send(SendMessage { content: "turn-1".into() })
            .await.unwrap().await.unwrap().unwrap();
        assert_eq!(r1, "reply-1");

        let r2 = addr
            .send(SendMessage { content: "turn-2".into() })
            .await.unwrap().await.unwrap().unwrap();
        assert_eq!(r2, "reply-2");

        let last = m.last_chat_call().unwrap();
        // user-1, assistant-1, user-2 = 3 messages on the second call.
        assert_eq!(last.len(), 3);
        assert_eq!(last[0].role, Role::User);
        assert_eq!(last[0].content, "turn-1");
        assert_eq!(last[1].role, Role::Assistant);
        assert_eq!(last[1].content, "reply-1");
        assert_eq!(last[2].role, Role::User);
        assert_eq!(last[2].content, "turn-2");
    }

    #[tokio::test]
    async fn stop_session_terminates() {
        let m = mock();
        let session = Session::new(m, "mock-chat".into(), Duration::from_secs(60), 3);
        let (addr, handle) = session.start("sess-stop").unwrap();

        addr.send(StopSession).await.unwrap().await.unwrap().unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn idle_timeout_terminates_session() {
        let m = mock();
        let session = Session::new(m, "mock-chat".into(), Duration::from_millis(100), 3);
        let (_addr, handle) = session.start("sess-idle").unwrap();

        // Wait long enough for the cron task to observe idle and terminate.
        let res = tokio::time::timeout(Duration::from_millis(800), handle).await;
        assert!(res.is_ok(), "session should have terminated on idle");
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Run LLM tests**

Run: `cargo test --lib llm::`
Expected: all pass.

- [ ] **Step 4: `cargo fmt`**

- [ ] **Step 5: Commit (covers tasks 6 + 7)**

```bash
git add src/llm.rs src/llm/session.rs
git commit -m "feat(llm): Arc<dyn Provider>, retry, cron-based idle timeout"
```

---

## Task 8: Wire `build_provider` in `main.rs`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Update the file**

Replace the body with:

```rust
use acktor::Actor;
use anyhow::Result;
use tracing::info;

use clawchorus::{
    config,
    llm::{LlmService, provider::build_provider},
    memory::manager::MemoryManager,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::Config::load()?;
    info!(
        host = %config.server.host,
        port = config.server.port,
        "ClawChorus starting"
    );
    info!(
        provider = %config.llm.provider,
        model = %config.llm.model,
        "LLM configuration"
    );

    let provider = build_provider(&config.llm)?;
    let llm = LlmService::new(config.llm, provider);
    let (llm_addr, _llm_handle) = llm.start("llm-service")?;

    let mm = MemoryManager::new(config.memory, llm_addr)?;
    let (_mm_addr, _mm_handle) = mm.start("memory-manager")?;

    info!("Memory Manager started");
    info!("Initialisation complete — HTTP server not yet started");

    tokio::signal::ctrl_c().await?;
    info!("Shutting down");

    Ok(())
}
```

- [ ] **Step 2: Build the workspace**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 4: `cargo fmt`**

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(llm): build provider at startup and inject into LlmService"
```

---

## Task 9: Parent-spec corrections

**Files:**
- Modify: `docs/superpowers/specs/clawchorus-design.md`
- Modify: `docs/superpowers/specs/memory-manager-design.md`

- [ ] **Step 1: Update `clawchorus-design.md`**

Find the LLM Sub-system bullet (currently around line 63):

```
- Two core capabilities: generate embeddings (spawns short-lived child per request), manage conversation sessions
```

Replace with:

```
- Two core capabilities: generate embeddings (handled inline on `LlmService` via a non-blocking future), manage conversation sessions
```

- [ ] **Step 2: Update `memory-manager-design.md`**

Find the LLM Service Messages table row for `Embed`. The cell currently reads:

```
| FileOp Actor / Search Actor / Synthesizer | Embed(Vec\<String\>) | Vec\<Embedding\> (spawns short-lived child actor per request) |
```

Replace with:

```
| FileOp Actor / Search Actor / Synthesizer | Embed(Vec\<String\>) | Vec\<Embedding\> |
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/clawchorus-design.md docs/superpowers/specs/memory-manager-design.md
git commit -m "docs: align embed wording with inline FutureMessageResult"
```

---

## Final Verification

- [ ] **Step 1: Full build**

Run: `cargo build`
Expected: success.

- [ ] **Step 2: Full test**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 3: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Format check**

Run: `cargo fmt --check`
Expected: clean.
