# LLM Service Actor — Component Design

## Overview

The LLM Service actor handles all outbound model API traffic for ClawChorus. It exposes two capabilities — embedding text batches and running multi-turn chat sessions — and isolates provider-specific HTTP details behind a single async `Provider` trait. It is a child of the top-level `Manager` supervisor, sibling to `MemoryManager` and the HTTP server.

## Module Layout

```
src/llm.rs                       LlmService actor + public messages (Embed, StartSession)
src/llm/config.rs                LlmConfig
src/llm/error.rs                 LlmError
src/llm/session.rs               Session actor + SendMessage / StopSession
src/llm/provider.rs              Provider trait, ChatMessage, ChatResponse, retry helper, build_provider
src/llm/provider/deepseek.rs     DeepSeek implementation (default)
```

## Actor Hierarchy

```
LlmService (long-lived, child of Manager)
  └── Session (per-conversation, long-lived until StopSession or idle)
```

No per-request child actor for embedding — `Embed` is handled with `FutureMessageResult` inline so `LlmService`'s mailbox stays responsive while the HTTP call is in flight.

## External Messages

Messages received by `LlmService`:

| Message      | Fields             | Reply                       |
| ------------ | ------------------ | --------------------------- |
| Embed        | texts: Vec<String> | EmbedResult / LlmError      |
| StartSession | —                  | Address<Session> / LlmError |

Messages received by `Session`:

| Message     | Fields          | Reply             |
| ----------- | --------------- | ----------------- |
| SendMessage | content: String | String / LlmError |
| StopSession | —               | Ok                |

A session is owned by the caller that opened it: the caller sends `StartSession` → receives an `Address<Session>` → sends `SendMessage` directly → sends `StopSession` when done. Sessions also self-terminate on idle (see Idle Timeout).

## Provider Trait

A plain trait, not an actor. `LlmService` constructs one `Arc<dyn Provider>` at startup and clones the `Arc` into each `Session` and each in-flight `Embed` future.

```rust
// src/llm/provider.rs

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Role { System, User, Assistant }

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub model: String,
    pub content: String,
}

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

pub fn build_provider(config: &LlmConfig) -> Result<Arc<dyn Provider>, LlmError>;
```

`build_provider` matches on `config.provider`. `"deepseek"` constructs `DeepSeekProvider`; unknown names return `LlmError::UnknownProvider`. Adding a new provider is one new file plus one match arm — no other code change.

The trait does not use the `async_trait` crate. Methods are declared as `fn` returning `Pin<Box<dyn Future<...> + Send + '_>>` so the trait is dyn-compatible without proc-macros.

## LlmService Actor

```rust
pub struct LlmService {
    config: LlmConfig,
    provider: Arc<dyn Provider>,
}

impl LlmService {
    pub fn new(config: LlmConfig, provider: Arc<dyn Provider>) -> Self { ... }
}

impl Actor for LlmService {
    type Context = Context<Self>;
    type Error = LlmError;
}
```

### Handlers

- **`Handler<Embed>`** returns `FutureMessageResult<Embed>`. Clones `self.provider` and `self.config.max_retries`, moves them into the returned future, and calls `retry(max_retries, || provider.embed(&texts))`. Returns `EmbedResult`.
- **`Handler<StartSession>`** returns `FutureMessageResult<StartSession>`. In the future, constructs `Session::new(provider.clone(), config.model.clone(), idle_timeout, max_retries)`, starts it with a generated label, returns the `Address<Session>`. Errors from `start` are mapped to `LlmError::Actor`.

`LlmService` itself is otherwise stateless.

## Session Actor

```rust
pub struct Session {
    provider: Arc<dyn Provider>,
    model: String,
    history: Vec<ChatMessage>,
    idle_timeout: Duration,
    last_activity: Instant,
    max_retries: u32,
}

impl Actor for Session {
    type Context = CronContext<Self>;
    type Error = LlmError;
}
```

### Handlers

- **`Handler<SendMessage>`** returns `FutureMessageResult<SendMessage>`. Pushes `ChatMessage { role: User, content }` into `history`, calls `retry(max_retries, || provider.chat(&history))`, pushes the assistant reply into `history`, updates `last_activity`, returns the reply string.
- **`Handler<StopSession>`** sends `Signal::Terminate` to its own address (matches current stub behavior).

The current `model` field on the provider response is propagated; `Session` stores only the configured `model` name to pass nothing extra on the wire — the provider is responsible for that.

### Idle Timeout

Implemented via acktor's `CronActor` trait — no detached tokio task.

```rust
impl CronActor for Session {
    async fn task(&mut self, ctx: &mut Self::Context) -> Result<Duration, LlmError> {
        let elapsed = self.last_activity.elapsed();
        if elapsed >= self.idle_timeout {
            trace!("Session idle, terminating");
            ctx.address().do_send(Signal::Terminate).await.ok();
            return Ok(Duration::from_secs(3600)); // effectively unused after Terminate
        }
        let remaining = self.idle_timeout.saturating_sub(elapsed);
        Ok(remaining.max(Duration::from_secs(1)))
    }
}
```

`SendMessage` updates `last_activity` so the next `task()` invocation sees fresh activity. acktor manages the cron loop and shuts it down cleanly when the actor stops.

### System Prompts

Not in v1. Callers wishing to seed a system instruction prepend it to the first user message. A future `SetSystemPrompt` message can promote this to a first-class field if needed.

## Retry Helper

Lives in `src/llm/provider.rs`, private to the module:

```rust
async fn retry<F, Fut, T>(max_attempts: u32, mut f: F) -> Result<T, LlmError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, LlmError>>;
```

Behavior:

- Up to `max_attempts` total (default 3, from `LlmConfig::max_retries`).
- Retries only on `LlmError::Transient`. Other errors return immediately.
- Backoff: 250ms, 500ms, 1s — capped at 1s, with full jitter (`rand::random::<f64>() * delay`).

## Error Types

`LlmError` is extended from its current two variants:

```rust
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("transient LLM error: {0}")]
    Transient(String),         // retryable: timeouts, 5xx, 429
    #[error("LLM provider error: {0}")]
    Provider(String),          // non-retryable: 4xx, parse, auth
    #[error("unknown LLM provider: {0}")]
    UnknownProvider(String),
    #[error("LLM actor messaging error: {0}")]
    Actor(String),
    #[error("LLM config error: {0}")]
    Config(String),            // missing API key env, invalid model
}
```

`MemoryError` already converts from `LlmError` via `From`; no change needed there.

## Config

`LlmConfig` gains three fields. Existing fields (`provider`, `api_key_env`, `model`, `embedding_model`, `embedding_dim`, `session_idle_timeout_secs`) are unchanged.

```rust
#[serde(default = "default_max_retries")]
pub max_retries: u32,                  // default 3

#[serde(default = "default_request_timeout_secs")]
pub request_timeout_secs: u64,         // default 30

#[serde(default = "default_base_url_deepseek")]
pub base_url: String,                  // default "https://api.deepseek.com"
```

The API key is read at `build_provider` time from `std::env::var(&config.api_key_env)`; an unset variable returns `LlmError::Config`.

## DeepSeek Provider

`src/llm/provider/deepseek.rs`. DeepSeek's API is OpenAI-compatible.

```rust
pub struct DeepSeekProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    chat_model: String,
    embedding_model: String,
}
```

`DeepSeekProvider::new(config)` builds a `reqwest::Client` with `timeout = Duration::from_secs(config.request_timeout_secs)`, reads the API key, and stores model names.

### embed

`POST {base_url}/v1/embeddings`

Request: `{ "model": embedding_model, "input": texts }`
Response: `{ "data": [{ "embedding": [f32; dim], "index": u32 }, ...], "model": String }`

Returns `EmbedResult { model, embeddings: data.iter().map(|d| Embedding(d.embedding.clone())).collect() }`. No internal batch-splitting in v1 — a single HTTP request per call. If a provider limit bites later, add splitting inside this method.

### chat

`POST {base_url}/v1/chat/completions`

Request: `{ "model": chat_model, "messages": [{role, content}, ...], "stream": false }`
Response: `{ "choices": [{ "message": { "content": String } }], "model": String }`

Returns `ChatResponse { model, content: choices[0].message.content }`.

### Error Mapping

Inside `DeepSeekProvider`:

- `reqwest::Error` where `is_timeout() || is_connect()` → `LlmError::Transient`.
- HTTP response where `status().is_server_error() || status() == 429` → `LlmError::Transient` with body excerpt.
- Other non-2xx → `LlmError::Provider` with status and body excerpt.
- JSON decode failure → `LlmError::Provider`.

## Wiring at Startup

`Manager::starting` calls `build_provider(&config.llm)?` once, then spawns `LlmService::new(config.llm.clone(), provider)`. The signature change to `LlmService::new` is the only call-site update outside the `llm` module. `MemoryManager` continues to receive the `Address<LlmService>` it does today.

## Testing

- **`provider::tests`** — a `MockProvider` (in-module `#[cfg(test)]`) records calls and returns canned responses or canned `LlmError`s. Used by `LlmService` and `Session` tests so they exercise the full actor path without HTTP.
- **`LlmService` tests:**
  - `Embed` returns the mock's vectors and the mock's model name.
  - `StartSession` returns a working `Session` address whose `SendMessage` reaches the mock provider.
- **`Session` tests:**
  - `SendMessage` appends user + assistant turns to history and returns the canned reply.
  - Multi-turn: two `SendMessage` calls — second call's `provider.chat` argument contains all four prior messages plus the new user turn.
  - `StopSession` terminates the actor.
  - Idle timeout: `idle_timeout = 100ms`, no activity → cron task terminates the actor within ~300ms.
- **`retry` tests:**
  - Transient → transient → success: 3 attempts, returns `Ok`.
  - `Provider` error: 1 attempt, returns `Err`.
  - 4 transient errors with `max_attempts = 3`: returns the last `Transient` error.
- **DeepSeek provider** — `wiremock`-based:
  - Happy-path embed: matches request body, returns canned embeddings, asserts decoded `EmbedResult`.
  - Happy-path chat: matches request body, returns canned completion.
  - 429 → 200 on retry: confirms `retry` actually re-issues the HTTP request when wrapped at the call site.

No live API calls in unit tests.

## Out of Scope for v1

Documented here so callers and future contributors do not assume them:

- Streaming chat responses (token-by-token).
- OpenAI / Anthropic providers (the trait is ready; implementations are not in this spec).
- Concurrent embedding batch splitting.
- System prompt as a first-class field on `StartSession`.
- Token accounting and cost tracking.
- Per-call model override (each `Session` is pinned to one model at start).

## Parent-Spec Corrections

This work updates two existing documents to match the inline `FutureMessageResult` approach for `Embed`:

- `docs/superpowers/specs/clawchorus-design.md` — the LLM Sub-system bullet currently reads "generate embeddings (spawns short-lived child per request), manage conversation sessions". Update to: "generate embeddings (handled inline on `LlmService` via a non-blocking future), manage conversation sessions".
- `docs/superpowers/specs/memory-manager-design.md` — the LLM Service Messages table's `Embed` row currently says "spawns short-lived child actor per request". Drop that parenthetical; the new wording is simply "Vec\<Embedding\>".
