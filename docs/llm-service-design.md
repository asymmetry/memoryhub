# LLM Service Actor — Component Design

## Overview

The LLM Service actor handles all outbound model API traffic for MemoryHub. It exposes two clearly separated capabilities — **embedding** text batches and **synthesizing** documents — and isolates provider-specific HTTP details behind two async traits, `Provider` (chat) and `EmbeddingProvider` (embeddings). It is a child of the top-level `Manager` supervisor, sibling to `MemoryManager` and the HTTP server.

Embedding and synthesis are kept apart at every level: separate child actors, separate messages, separate provider traits. This split also lets a deployment pair a chat-only vendor (e.g. DeepSeek) with a different embeddings vendor (e.g. OpenAI).

## Module Layout

```
src/llm.rs                       LlmService actor + public messages (Embed, Synthesize)
src/llm/config.rs                LlmConfig
src/llm/error.rs                 LlmError
src/llm/embedder.rs              Embedder actor
src/llm/synthesis.rs             SynthesisTask actor + SynthesisTarget, SourceDoc, Synthesize
src/llm/template.rs              prompt template resolution, hot-reload, default seeding
src/llm/prompts/per_user.md      embedded default per-user prompt
src/llm/prompts/global.md        embedded default global prompt
src/llm/provider.rs              Provider + EmbeddingProvider traits, ChatMessage, ChatResponse, retry helper, build_providers
src/llm/provider/deepseek.rs     DeepSeek implementation (chat only — DeepSeek has no embeddings endpoint)
src/llm/provider/openai.rs       OpenAI implementation (chat + embeddings)
src/llm/provider/mock.rs         MockProvider (gated by `cfg(test)` or `feature = "_test"`)
```

## Actor Hierarchy

```
LlmService (long-lived, child of Manager) — single entry point
  ├── Embedder (long-lived) — handles Embed
  └── SynthesisTask (long-lived, one per SynthesisTarget; idle-terminates)
```

- `LlmService` constructs the `Arc<dyn Provider>` and `Arc<dyn EmbeddingProvider>` in `Actor::post_start`. When `provider == embedding_provider` and that provider impl supports both roles (currently `openai` and `mock`), a single instance is shared between both Arcs so HTTP clients and credential reads aren't duplicated.
- `Embedder` is spawned once at startup with the embedding provider.
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
    Global,         // cross-user synthesis
}

pub struct SourceDoc {
    pub name: String,     // a label for the document, e.g. its rel_path
    pub content: String,
}
```

`SynthesisTarget` selects **both** the long-lived task to route to and the prompt template kind (`User` → `per_user`, `Global` → `global`). Callers never hold a task address — `LlmService` owns task routing and lifecycle. `Embedder` and `SynthesisTask` have no externally-visible messages.

## Provider Traits

Plain traits, not actors. `LlmService` constructs one `Arc<dyn Provider>` and one `Arc<dyn EmbeddingProvider>` at startup; the chat Arc is cloned into each `SynthesisTask`, the embedding Arc into the `Embedder`.

```rust
// src/llm/provider.rs

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>>;
}

pub trait EmbeddingProvider: Send + Sync + 'static {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>>;
}

pub fn build_providers(
    config: &LlmConfig,
) -> Result<(Arc<dyn Provider>, Arc<dyn EmbeddingProvider>), LlmError>;
```

`build_providers` matches on `config.provider` for the chat Arc and `config.embedding_provider` for the embedding Arc. Unknown names return `LlmError::UnknownProvider`. When both names are equal and the impl serves both roles (e.g. `openai`, `mock`), one instance is constructed and shared. Adding a new provider is one new file plus one match arm per role it supports.

The traits do not use the `async_trait` crate. Methods are declared as `fn` returning `Pin<Box<dyn Future<...> + Send + '_>>` so the traits are dyn-compatible without proc-macros.

## LlmService Actor

```rust
pub struct LlmService {
    config: LlmConfig,
    provider: Option<Arc<dyn Provider>>,         // populated in post_start
    embedder: Option<Address<Embedder>>,          // populated in post_start
    embedder_handle: Option<JoinHandle<()>>,
    tasks: HashMap<SynthesisTarget, Address<SynthesisTask>>,
}

impl Actor for LlmService {
    type Context = Context<Self>;
    type Error = LlmError;
}
```

`LlmService::new` returns the bare struct; provider construction, default-template seeding (`template::write_default_templates`), and `Embedder` spawning all happen in `Actor::post_start`. `post_stop` terminates the `Embedder` cleanly. The `Option` wrappers exist only because the providers can't be built synchronously in `new`; once `post_start` completes they are always `Some` for the actor's lifetime.

### Handlers

- **`Handler<Embed>`** returns `FutureMessageResult<Embed>`. Clones the `Embedder` address and forwards the `Embed` message to it in the returned future, so `LlmService`'s mailbox stays responsive.
- **`Handler<Synthesize>`** returns `FutureMessageResult<Synthesize>`. Inline (before returning the future), it gets-or-spawns the `SynthesisTask` for `target`: if the map has no entry it spawns a fresh task and stores it. The returned future forwards the `Synthesize` message to that task and awaits the result. When a new task is spawned, a small `tokio::spawn` awaits its `JoinHandle` and sends a private `SynthesisTaskTerminated { target }` message back to `LlmService` once the task exits.
- **`Handler<SynthesisTaskTerminated>`** (private) removes the stale `tasks` entry on idle termination, so the next `Synthesize` for that target spawns a fresh task.

## Embedder Actor

```rust
pub struct Embedder {
    provider: Arc<dyn EmbeddingProvider>,
    max_retries: u32,
}
```

- **`Handler<Embed>`** returns `FutureMessageResult<Embed>`. Clones `provider` and `max_retries` into the returned future and calls `retry(max_retries, || provider.embed(&texts))`. Returns `EmbedResult`.

`Embedder` is otherwise stateless. It exists as its own actor so embedding is fully isolated from synthesis.

## SynthesisTask Actor

One long-lived actor per `SynthesisTarget`. It owns the conversation context for that target so successive cool-down cycles refine a synthesis instead of rebuilding it.

```rust
pub struct SynthesisTask {
    config: LlmConfig,         // supplies prompts_dir, max_retries, idle timeout, context budget, model
    provider: Arc<dyn Provider>,
    kind: TemplateKind,        // per_user | global, derived from the target
    history: Vec<ChatMessage>, // accumulated User/Assistant turns, no system message
    last_activity: Instant,
}

impl Actor for SynthesisTask {
    type Context = CronContext<Self>;
    type Error = LlmError;
}
```

### Handler<Synthesize>

Returns `Result<String, LlmError>` (synchronous, not `FutureMessageResult`, because the task processes one synthesis at a time and ordering matters). Steps:

1. Update `last_activity` and snapshot `history.len()` as `restore_len` so a failed provider call can be rolled back to its pre-call state.
2. **Hot-reload** the prompt template for `(provider.name(), kind)` from `prompts_dir` (see Prompt Templates). It becomes the `System` message — re-read every call so prompt edits take effect with no restart.
3. If `history` is empty and `prior_summary` is `Some`, push a `User` turn carrying the summary as the task's starting context. (`history` is empty for a freshly spawned task, after a restart, and after a threshold reset — this single step covers all three.)
4. Push the `sources` as a single `User` turn (one Markdown block per source, headed by its `name`).
5. Build the call list as `[system] ++ history` and call `retry(max_retries, || provider.chat(&messages))`. On error, truncate `history` back to `restore_len` and return the error.
6. Push the assistant reply into `history`; update `last_activity`.
7. If the total size of `history` now exceeds `synthesis_context_max_chars`, clear `history`. The next `Synthesize` reseeds from its `prior_summary` — bounding context without losing state, since the summary is the distilled form of everything fed so far.
8. Return the reply string.

The reply is the synthesized document. The caller (Synthesizer) writes it to disk and indexes it; `SynthesisTask` never touches Storage.

### Idle Timeout

Implemented via acktor's `CronActor` trait (no detached tokio task): the cron callback terminates the actor once `last_activity` exceeds `idle_timeout`, otherwise reschedules itself for the remaining interval. A terminated task drops its in-memory context; `LlmService` removes it from `tasks` (see Handlers), so the next `Synthesize` for that target spawns a fresh task that reseeds from `prior_summary`.

## Prompt Templates

Synthesis prompts live as Markdown files on disk so they can be changed without recompiling.

```
{prompts_dir}/
  per_user.md            # default, used by any provider
  global.md
  deepseek/
    per_user.md          # optional override, used only when provider name == "deepseek"
    global.md
```

- **Resolution** (`src/llm/template.rs::load_template`):
  1. `{prompts_dir}/{provider_name}/{kind}.md` if it exists,
  2. else `{prompts_dir}/{kind}.md` if it exists,
  3. else the embedded default compiled into the binary via `include_str!` from `src/llm/prompts/{kind}.md`.
- **Hot-reload:** any on-disk file is read fresh on every `Synthesize` call. Editing a template takes effect on the next synthesis with no restart.
- A template file is **static text** — the whole synthesis system prompt for that kind. There is no placeholder/interpolation mechanism; source documents are passed as chat messages, not spliced into the template.
- **Default seeding:** on `LlmService::post_start`, `template::write_default_templates(prompts_dir)` creates `prompts_dir` if needed and writes the embedded defaults to `{prompts_dir}/per_user.md` and `{prompts_dir}/global.md` **only if those files do not already exist**, so user edits are preserved across restarts. Failure to seed is logged as a warning and does not fail startup — `load_template` still falls back to the embedded default.

`prompts_dir` is configurable (see Config), default `~/.memoryhub/prompts` (falling back to a temp-dir path if the home directory cannot be resolved).

## Retry Helper

`retry(max_attempts, f)` in `src/llm/provider.rs`, private to the module, wraps a fallible async call:

- Up to `max_attempts` total (default 3, from `LlmConfig::max_retries`).
- Retries only on `LlmError::Transient`; other errors return immediately.
- Backoff 250ms / 500ms / 1s — capped at 1s, with full jitter.

## Error Types

```rust
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("LLM config error: {0}")]
    Config(String),                              // missing API key env, invalid model
    #[error("unknown LLM provider: {0}")]
    UnknownProvider(String),
    #[error("LLM provider error: {0}")]
    Provider(String),                            // non-retryable: 4xx, parse, auth
    #[error("could not load prompt template")]
    LoadTemplate(#[from] std::io::Error),
    #[error("could not write default prompt templates")]
    WriteDefaultTemplates(std::io::Error),
    #[error("transient LLM error: {0}")]
    Transient(String),                           // retryable: timeouts, 5xx, 429
    #[error("could not send message")]
    SendError(#[source] BoxError),               // wraps acktor SendError<M>
    #[error("could not receive message")]
    RecvError(#[from] RecvError),
}
```

`SendError<M>` is converted into `LlmError::SendError` via a blanket `impl<M> From<SendError<M>> for LlmError`. `MemoryError` already converts from `LlmError` via `From`; no change needed there.

## Config

`LlmConfig`:

```rust
pub provider: String,                          // e.g. "deepseek"
pub embedding_provider: String,                // e.g. "openai"
pub api_key_env: String,                       // env var name for chat API key
pub embedding_api_key_env: String,             // env var name for embedding API key
pub model: String,
pub embedding_model: String,

#[serde(default)]
pub embedding_dim: Option<usize>,              // pin to avoid surprises; None = auto-detect

#[serde(default = "default_prompts_dir")]
pub prompts_dir: PathBuf,                      // default "~/.memoryhub/prompts"

#[serde(default = "default_synthesis_idle_timeout_secs")]
pub synthesis_idle_timeout_secs: u64,          // SynthesisTask idle timeout, default 300

#[serde(default = "default_synthesis_context_max_chars")]
pub synthesis_context_max_chars: usize,        // reset threshold, default 200_000

#[serde(default = "default_max_retries")]
pub max_retries: u32,                          // default 3

#[serde(default = "default_request_timeout_secs")]
pub request_timeout_secs: u64,                 // default 30

#[serde(default = "default_base_url")]
pub base_url: String,                          // chat provider base, default "https://api.deepseek.com"

#[serde(default = "default_embedding_base_url")]
pub embedding_base_url: String,                // embedding provider base, default "https://api.openai.com/v1"
```

Chat and embedding providers each have their own provider name, env-var name, and base URL because the default deployment pairs DeepSeek (chat) with OpenAI (embeddings). When chat and embedding providers are the same (e.g. both `openai`), `build_providers` shares one instance — see *Provider Traits*. The `OpenAiProvider` itself uses a single `base_url` and `api_key` internally (whichever pair was passed in at construction) for both its `chat` and `embed` calls. API keys are read at `build_providers` time from `std::env::var(&config.api_key_env)` / `std::env::var(&config.embedding_api_key_env)`; an unset variable returns `LlmError::Config`.

## DeepSeek Provider

`src/llm/provider/deepseek.rs`. DeepSeek's chat API is OpenAI-compatible. DeepSeek does **not** expose an embeddings endpoint, so `DeepSeekProvider` implements only `Provider`, not `EmbeddingProvider`.

```rust
pub struct DeepSeekProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    chat_model: String,
}
```

`DeepSeekProvider::new(config)` builds a `reqwest::Client` with `timeout = Duration::from_secs(config.request_timeout_secs)`, reads the chat API key from `config.api_key_env`, and stores `config.base_url` and `config.model`. `name()` returns `"deepseek"`.

### chat

`POST {base_url}/chat/completions`

Request: `{ "model": chat_model, "messages": [{role, content}, ...], "stream": false }`
Response: `{ "choices": [{ "message": { "content": String } }], "model": String }`

Returns `ChatResponse { model, content: choices[0].message.content }`.

## OpenAI Provider

`src/llm/provider/openai.rs`. Implements **both** `Provider` and `EmbeddingProvider`.

```rust
pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    chat_model: String,
    embedding_model: String,
}
```

`OpenAiProvider::new(config)` reads `config.api_key_env` for the API key, stores `config.base_url` (used for both endpoints) and the two model names, and builds the shared `reqwest::Client`. `name()` returns `"openai"`.

When `build_providers` constructs an `OpenAiProvider` for the embedding role only (chat is DeepSeek), the same `LlmConfig` is passed in, so the OpenAI instance uses `config.base_url` and `config.api_key_env`. Deployments that pair DeepSeek+OpenAI must therefore set `base_url` / `api_key_env` to the OpenAI values *if* OpenAI is the only provider, or supply `embedding_base_url` / `embedding_api_key_env` and rework which Arc reads which — see the in-code `build_providers` for the current wiring.

### embed

`POST {base_url}/embeddings`

Request: `{ "model": embedding_model, "input": texts }`
Response: `{ "model": String, "data": [{ "embedding": [f32; dim] }, ...] }`

Returns `EmbedResult { model, embeddings }`. No internal batch-splitting in v1 — a single HTTP request per call.

### chat

`POST {base_url}/chat/completions`

Request and response shape identical to DeepSeek.

### Error Mapping (both providers)

- `reqwest::Error` where `is_timeout() || is_connect()` → `LlmError::Transient`.
- HTTP response where `status().is_server_error() || status() == 429` → `LlmError::Transient` with body excerpt (truncated to 512 chars).
- Other non-2xx → `LlmError::Provider` with status and body excerpt.
- JSON decode failure → `LlmError::Provider`.

## Wiring at Startup

`Manager` constructs `LlmService::new(config.llm.clone())` and spawns it; provider construction happens inside `LlmService::post_start` via `build_providers`. `MemoryManager` continues to receive the `Address<LlmService>` it does today.

## Testing

A `MockProvider` in `src/llm/provider/mock.rs` (gated by `cfg(test)` or `feature = "_test"`) implements both `Provider` and `EmbeddingProvider`, records calls, and returns canned responses or `LlmError`s, so actor tests run the full path without HTTP.

- **Embedder / LlmService:** `Embed` is forwarded and returns the mock's result; `Synthesize` spawns one task per target and reuses it across calls (the mock sees accumulated history).
- **SynthesisTask:** `prior_summary` seeds an empty history before the sources; turns accumulate across cycles; a tiny `synthesis_context_max_chars` triggers a reset that reseeds on the next call; missing on-disk template falls back to the embedded default; idle timeout terminates the actor.
- **template:** provider-specific file wins over default; on-disk default wins over embedded default; embedded default used when neither file exists; `write_default_templates` seeds missing files but preserves edited ones.
- **retry:** retries transient errors up to `max_attempts`, returns immediately on non-transient, surfaces the last transient error when exhausted.
- **DeepSeek / OpenAI providers:** `wiremock` happy-path chat (and embed, for OpenAI), plus status-code mapping checks.

No live API calls in unit tests.

## Out of Scope for v1

- Streaming chat responses (token-by-token).
- Anthropic / other providers (the traits are ready; implementations are not in this spec).
- Concurrent embedding batch splitting.
- A generic externally-driven multi-turn chat API — synthesis is the only chat consumer and is fully encapsulated.
- Placeholder/variable interpolation in prompt templates.
- Persisting `SynthesisTask` context across process restarts (recovery is via `prior_summary`).
- Token accounting and cost tracking.
- Per-call model override (each `SynthesisTask` is pinned to one model).
