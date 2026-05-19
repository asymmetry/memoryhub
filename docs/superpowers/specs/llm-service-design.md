# LLM Service Actor — Component Design

## Overview

The LLM Service actor handles all outbound model API traffic for ClawChorus. It exposes two clearly separated capabilities — **embedding** text batches and **synthesizing** documents — and isolates provider-specific HTTP details behind a single async `Provider` trait. It is a child of the top-level `Manager` supervisor, sibling to `MemoryManager` and the HTTP server.

Embedding and synthesis are kept apart at every level: separate child actors, separate messages, separate provider methods.

## Module Layout

```
src/llm.rs                       LlmService actor + public messages (Embed, Synthesize)
src/llm/config.rs                LlmConfig
src/llm/error.rs                 LlmError
src/llm/embedder.rs              Embedder actor
src/llm/synthesis.rs             SynthesisTask actor + SynthesisTarget, SourceDoc
src/llm/template.rs              prompt template resolution + loading
src/llm/provider.rs              Provider trait, ChatMessage, ChatResponse, retry helper, build_provider
src/llm/provider/deepseek.rs     DeepSeek implementation (default)
```

## Actor Hierarchy

```
LlmService (long-lived, child of Manager) — single entry point
  ├── Embedder (long-lived) — handles Embed
  └── SynthesisTask (long-lived, one per SynthesisTarget; idle-terminates)
```

- `LlmService` constructs the `Arc<dyn Provider>` at startup and clones it into each child.
- `Embedder` is spawned once at startup.
- A `SynthesisTask` is spawned lazily on the first `Synthesize` for a given target and kept alive across cool-down cycles so it preserves conversation context. It self-terminates after an idle period; the next `Synthesize` for that target spawns a fresh one.

## External Messages

Messages received by `LlmService`:

| Message    | Fields                                                                              | Reply                  |
| ---------- | ----------------------------------------------------------------------------------- | ---------------------- |
| Embed      | texts: Vec\<String\>                                                                | EmbedResult / LlmError |
| Synthesize | target: SynthesisTarget, prior_summary: Option\<String\>, sources: Vec\<SourceDoc\> | String / LlmError      |

```rust
pub enum SynthesisTarget {
    User(String),   // per-user synthesis; the String is the username
    Overall,        // cross-user synthesis
}

pub struct SourceDoc {
    pub name: String,     // a label for the document, e.g. its rel_path
    pub content: String,
}
```

`SynthesisTarget` selects **both** the long-lived task to route to and the prompt template kind (`User` → `per_user`, `Overall` → `overall`). Callers never hold a task address — `LlmService` owns task routing and lifecycle. `Embedder` and `SynthesisTask` have no externally-visible messages.

## Provider Trait

A plain trait, not an actor. `LlmService` constructs one `Arc<dyn Provider>` at startup and clones the `Arc` into the `Embedder` and each `SynthesisTask`.

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
    /// Provider name, e.g. "deepseek". Used to resolve provider-specific
    /// prompt templates.
    fn name(&self) -> &str;

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
    embedder: Address<Embedder>,
    tasks: HashMap<SynthesisTarget, Address<SynthesisTask>>,
}

impl Actor for LlmService {
    type Context = Context<Self>;
    type Error = LlmError;
}
```

`LlmService::new` builds the actor; `Embedder` is started in `Actor::starting`.

### Handlers

- **`Handler<Embed>`** returns `FutureMessageResult<Embed>`. Clones the `Embedder` address and forwards the `Embed` message to it in the returned future, so `LlmService`'s mailbox stays responsive.
- **`Handler<Synthesize>`** returns `FutureMessageResult<Synthesize>`. Inline (before returning the future), it gets-or-spawns the `SynthesisTask` for `target`: if the map has no entry it spawns a fresh task and stores it. The returned future forwards the `Synthesize` message to that task and awaits the result.

`LlmService` supervises its `SynthesisTask` children; when one terminates on idle, `LlmService` removes its entry from `tasks`, so the next `Synthesize` for that target spawns a fresh task.

## Embedder Actor

```rust
pub struct Embedder {
    provider: Arc<dyn Provider>,
    max_retries: u32,
}
```

- **`Handler<Embed>`** returns `FutureMessageResult<Embed>`. Clones `provider` and `max_retries` into the returned future and calls `retry(max_retries, || provider.embed(&texts))`. Returns `EmbedResult`.

`Embedder` is otherwise stateless. It exists as its own actor so embedding is fully isolated from synthesis.

## SynthesisTask Actor

One long-lived actor per `SynthesisTarget`. It owns the conversation context for that target so successive cool-down cycles refine a synthesis instead of rebuilding it.

```rust
pub struct SynthesisTask {
    provider: Arc<dyn Provider>,
    model: String,
    kind: TemplateKind,        // per_user | overall, derived from the target
    prompts_dir: PathBuf,
    history: Vec<ChatMessage>, // accumulated User/Assistant turns, no system message
    idle_timeout: Duration,
    last_activity: Instant,
    max_retries: u32,
    context_max_chars: usize,  // reset threshold
}

impl Actor for SynthesisTask {
    type Context = CronContext<Self>;
    type Error = LlmError;
}
```

### Handler<Synthesize>

Returns `FutureMessageResult<Synthesize>`. Steps:

1. **Hot-reload** the prompt template for `(provider.name(), kind)` from `prompts_dir` (see Prompt Templates). It becomes the `System` message — re-read every call so prompt edits take effect with no restart.
2. If `history` is empty and `prior_summary` is `Some`, push a `User` turn carrying the summary as the task's starting context. (`history` is empty for a freshly spawned task, after a restart, and after a threshold reset — this single step covers all three.)
3. Push the `sources` as a `User` turn.
4. Call `retry(max_retries, || provider.chat(&[system] ++ history))`.
5. Push the assistant reply into `history`; update `last_activity`.
6. If the total size of `history` now exceeds `context_max_chars`, clear `history`. The next `Synthesize` reseeds from its `prior_summary` — bounding context without losing state, since the summary is the distilled form of everything fed so far.
7. Return the reply string.

The reply is the synthesized document. The caller (Synthesizer) writes it to disk and indexes it; `SynthesisTask` never touches Storage.

### Idle Timeout

Implemented via acktor's `CronActor` trait (no detached tokio task): the cron callback terminates the actor once `last_activity` exceeds `idle_timeout`, otherwise reschedules itself for the remaining interval. A terminated task drops its in-memory context; `LlmService` removes it from `tasks` (see Handlers), so the next `Synthesize` for that target spawns a fresh task that reseeds from `prior_summary`.

## Prompt Templates

Synthesis prompts live as Markdown files on disk so they can be changed without recompiling.

```
{prompts_dir}/
  per_user.md            # default, used by any provider
  overall.md
  deepseek/
    per_user.md          # optional override, used only when provider name == "deepseek"
    overall.md
```

- **Resolution** (`src/llm/template.rs`): for `(provider_name, kind)`, try `{prompts_dir}/{provider_name}/{kind}.md`; if absent, fall back to `{prompts_dir}/{kind}.md`.
- **Hot-reload:** the file is read fresh on every `Synthesize` call. Editing a template takes effect on the next synthesis with no restart.
- A template file is **static text** — the whole synthesis system prompt for that kind. There is no placeholder/interpolation mechanism; source documents are passed as chat messages, not spliced into the template.
- A missing template (no provider override and no default) returns `LlmError::Config`.

`prompts_dir` is configurable (see Config), default `./prompts`.

## Retry Helper

`retry(max_attempts, f)` in `src/llm/provider.rs`, private to the module, wraps a fallible async call:

- Up to `max_attempts` total (default 3, from `LlmConfig::max_retries`).
- Retries only on `LlmError::Transient`; other errors return immediately.
- Backoff 250ms / 500ms / 1s — capped at 1s, with full jitter.

## Error Types

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
    Config(String),            // missing API key env, invalid model, missing prompt template
}
```

`MemoryError` already converts from `LlmError` via `From`; no change needed there.

## Config

`LlmConfig`:

```rust
pub provider: String,                  // e.g. "deepseek"
pub api_key_env: String,
pub model: String,
pub embedding_model: String,
pub embedding_dim: u32,

#[serde(default = "default_synthesis_idle_timeout_secs")]
pub synthesis_idle_timeout_secs: u64,  // SynthesisTask idle timeout, default 300

#[serde(default = "default_prompts_dir")]
pub prompts_dir: PathBuf,              // default "./prompts"

#[serde(default = "default_synthesis_context_max_chars")]
pub synthesis_context_max_chars: usize, // reset threshold, default 200_000

#[serde(default = "default_max_retries")]
pub max_retries: u32,                  // default 3

#[serde(default = "default_request_timeout_secs")]
pub request_timeout_secs: u64,         // default 30

#[serde(default = "default_base_url_deepseek")]
pub base_url: String,                  // default "https://api.deepseek.com"
```

`session_idle_timeout_secs` is renamed to `synthesis_idle_timeout_secs` since there is no longer a generic chat session. The API key is read at `build_provider` time from `std::env::var(&config.api_key_env)`; an unset variable returns `LlmError::Config`.

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

`DeepSeekProvider::new(config)` builds a `reqwest::Client` with `timeout = Duration::from_secs(config.request_timeout_secs)`, reads the API key, and stores model names. `name()` returns `"deepseek"`.

### embed

`POST {base_url}/v1/embeddings`

Request: `{ "model": embedding_model, "input": texts }`
Response: `{ "data": [{ "embedding": [f32; dim], "index": u32 }, ...], "model": String }`

Returns `EmbedResult { model, embeddings: data.iter().map(|d| Embedding(d.embedding.clone())).collect() }`. No internal batch-splitting in v1 — a single HTTP request per call.

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

`Manager::starting` calls `build_provider(&config.llm)?` once, then spawns `LlmService::new(config.llm.clone(), provider)`. `MemoryManager` continues to receive the `Address<LlmService>` it does today.

## Testing

A `MockProvider` (`#[cfg(test)]` in `provider.rs`) records calls and returns canned responses or `LlmError`s, so actor tests run the full path without HTTP.

- **Embedder / LlmService:** `Embed` is forwarded and returns the mock's result; `Synthesize` spawns one task per target and reuses it across calls (the mock sees accumulated history).
- **SynthesisTask:** `prior_summary` seeds an empty history before the sources; turns accumulate across cycles; a tiny `context_max_chars` triggers a reset that reseeds on the next call; idle timeout terminates the actor.
- **template:** provider-specific file wins over default; default used when no override; neither present → `LlmError::Config`.
- **retry:** retries transient errors up to `max_attempts`, returns immediately on non-transient, surfaces the last transient error when exhausted.
- **DeepSeek provider:** `wiremock` happy-path embed and chat, plus 429 → 200 retry.

No live API calls in unit tests.

## Out of Scope for v1

- Streaming chat responses (token-by-token).
- OpenAI / Anthropic providers (the trait is ready; implementations are not in this spec).
- Concurrent embedding batch splitting.
- A generic externally-driven multi-turn chat API — synthesis is the only chat consumer and is fully encapsulated.
- Placeholder/variable interpolation in prompt templates.
- Persisting `SynthesisTask` context across process restarts (recovery is via `prior_summary`).
- Token accounting and cost tracking.
- Per-call model override (each `SynthesisTask` is pinned to one model).
