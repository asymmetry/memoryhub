# LLM Service Actor — Component Design

## Overview

The LLM Service actor handles all outbound model API traffic for MemoryHub. It exposes two capabilities — **embedding** text batches and **synthesizing** documents — and hides provider-specific HTTP behind two traits, `Provider` (chat) and `EmbeddingProvider` (embeddings). It is a child of `Manager`, sibling to `MemoryManager` and the HTTP server.

Embedding and synthesis are kept apart at every level: separate child actors, separate messages, separate provider traits. This lets a deployment pair a chat-only vendor (e.g. DeepSeek) with a different embeddings vendor (e.g. OpenAI), which is the default configuration.

## Actor Hierarchy

```
LlmService (long-lived, child of Manager) — single entry point
  ├── Embedder (long-lived) — handles Embed
  └── SynthesisTask (long-lived, one per SynthesisTarget; idle-terminates)
```

`LlmService` is the only externally addressable actor; callers never hold an `Embedder` or `SynthesisTask` address, so it owns all routing and lifecycle. Providers are built in `post_start` (not `new`) because they can't be constructed synchronously. `Embedder` is spawned once. A `SynthesisTask` is spawned lazily on the first `Synthesize` for a target and kept alive across cool-down cycles so it preserves conversation context; it self-terminates after an idle period and the next call respawns it.

## External Messages

`LlmService` receives two messages; `Embedder` and `SynthesisTask` have none that are externally visible.

| Message      | Fields                               | Reply                         |
| ------------ | ------------------------------------ | ----------------------------- |
| `Embed`      | `texts`                              | embeddings / `LlmError`       |
| `Synthesize` | `target`, `prior_summary`, `sources` | synthesized text / `LlmError` |

`target` is a `SynthesisTarget` — `User(username)` or `Global`. It selects **both** the long-lived task to route to and the prompt template kind (`User` → `per_user`, `Global` → `global`). `Embed` is forwarded to the `Embedder` from inside the returned future so the `LlmService` mailbox stays responsive; `Synthesize` gets-or-spawns the target's task, then forwards. When a task is spawned, `LlmService` watches its `JoinHandle` and removes the map entry on idle termination so the next call respawns cleanly.

## Provider Traits

`Provider` (chat) and `EmbeddingProvider` (embeddings) are plain async traits, not actors. They are declared as methods returning `Pin<Box<dyn Future + Send>>` rather than using `async_trait`, so they stay dyn-compatible without proc-macros. `LlmService` builds one `Arc<dyn Provider>` (cloned into each `SynthesisTask`) and one `Arc<dyn EmbeddingProvider>` (cloned into the `Embedder`).

`build_providers` matches `config.provider` for the chat Arc and `config.embedding_provider` for the embedding Arc; an unknown name returns `LlmError::UnknownProvider`. When both names are equal and the impl serves both roles (`openai`, `mock`), a single instance is built and shared so HTTP clients and credential reads aren't duplicated. Adding a provider is one new file plus a match arm per role it supports.

### Per-role credentials

A provider instance is built **per role**, and each role reads its own config:

- **Chat role** reads `api_key_env`, `base_url`, `model`.
- **Embedding role** reads `embedding_api_key_env`, `embedding_base_url`, `embedding_model`.
- A provider serving **both** roles is treated as a chat provider (reads the chat fields), with the one instance shared.

This is what makes the default DeepSeek-chat + OpenAI-embeddings split work: each side reads its own URL and key. The two sets of fields exist precisely because the chat and embedding vendors differ in the default deployment.

## SynthesisTask

One long-lived actor per target, owning the conversation history for that target so successive cool-down cycles refine a synthesis instead of rebuilding it from scratch. Each `Synthesize`:

1. Hot-reloads the prompt template (re-read every call so edits take effect with no restart) as the system message.
2. If history is empty and a `prior_summary` is supplied, seeds history with it — covering a freshly spawned, restarted, or just-reset task in one step.
3. Appends the `sources` as one user turn, calls the provider with `[system] ++ history`, and on success appends the reply. On failure, history is rolled back to its pre-call length and the error is returned.
4. If history exceeds `synthesis_context_max_chars`, it is cleared; the next call reseeds from `prior_summary`. This bounds context without losing state, since the summary is the distilled form of everything fed so far.

Synthesis processes one request at a time (ordering matters), so its handler is synchronous rather than a future. The reply is the synthesized document; the caller (Synthesizer) writes and indexes it — `SynthesisTask` never touches Storage.

Idle timeout uses acktor's `CronActor` (no detached Tokio task): the cron callback terminates the actor once it has been idle past `synthesis_idle_timeout_secs`, otherwise reschedules. Context is intentionally **not** persisted across process restarts — recovery is via `prior_summary`, which is simpler and good enough since the summary already captures prior state.

## Prompt Templates

Synthesis prompts are Markdown files on disk so they can be changed without recompiling. A template is static text — the whole system prompt for that kind; there is no interpolation, because source documents are passed as chat messages instead. Resolution per `(provider_name, kind)`:

1. `{prompts_dir}/{provider}/{kind}.md` (provider-specific override), else
2. `{prompts_dir}/{kind}.md`, else
3. the default compiled into the binary.

On startup the embedded defaults are written to `{prompts_dir}` **only if those files don't already exist**, so user edits survive restarts. A seeding failure is logged, not fatal — resolution still falls back to the embedded default.

## Retry

Provider calls are wrapped in a retry helper: up to `max_retries` attempts, retrying **only** `LlmError::Transient` (timeouts, 5xx, 429) and returning other errors immediately. Backoff bases are 250ms / 500ms / 1s, each scaled by a random factor in `[0.5, 1.0)` to spread out concurrent retries.

## Errors

`LlmError` distinguishes config problems (unset key env, unknown provider), non-retryable provider errors (4xx, parse, auth), retryable `Transient` errors, template I/O, and actor send/recv failures. The `Transient` vs non-transient split is the contract the retry helper depends on. `MemoryError` already converts from `LlmError` via `From`.

## Providers

- **DeepSeek** — OpenAI-compatible chat API; implements only `Provider` (DeepSeek has no embeddings endpoint). Default chat provider.
- **OpenAI** — implements both `Provider` and `EmbeddingProvider`; default embedding provider. Two role-specific constructors share one private builder so each role picks up the right key/URL/model.
- **Mock** — gated by `cfg(test)` / `feature = "_test"`; records calls and returns canned responses so actor tests run the full path without HTTP.

Error mapping is shared across the real providers: timeouts/connect errors and 5xx/429 → `Transient`; other non-2xx and JSON decode failures → non-retryable `Provider` (with a truncated body excerpt).

## Out of Scope (v1)

- Streaming chat responses
- Concurrent embedding batch splitting
- A general multi-turn chat API (synthesis is the only chat consumer)
- Persisting `SynthesisTask` context across restarts
- Token accounting / cost tracking, per-call model override
