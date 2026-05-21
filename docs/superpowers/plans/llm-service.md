# LLM Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the LLM service to its final form — two cleanly separated capabilities (embedding and synthesis), synthesis owning prompt engineering and conversation context behind a `Synthesize` message, and the Synthesizer rewired to feed only changed files. This plan supersedes the earlier LLM service plan and is written against the current codebase, which already has the stub-replacement implementation in place.

**Architecture:** `LlmService` stays the single entry point. It spawns one long-lived `Embedder` child and one long-lived `SynthesisTask` child per synthesis target (per-user / global). `SynthesisTask` preserves conversation context across cool-down cycles, hot-reloads Markdown prompt templates from disk, and reseeds from a caller-supplied prior summary on cold start or context reset. The Synthesizer no longer drives LLM sessions — it reads changed files and prior summaries from Storage, calls `Synthesize`, and writes results back.

**Tech Stack:** Rust 2024, `acktor` actor framework, `tokio`, `reqwest`. Tests use the in-crate `MockProvider` (enabled by the `_test` feature via dev-dependencies).

---

## Background for the implementer

- Modules use the `foo.rs` + `foo/` layout (no `mod.rs`).
- Run `cargo fmt` after modifying any `.rs` file (project rule).
- Every `Handler::handle` starts with `trace!("Handle command {:?}", msg);`.
- Log values are embedded in the message prose, not as structured `key=value` fields.
- `acktor` actor round-trip: `addr.send(msg).await` → `Result<ResponseFuture, SendError>`; `.await` on the `ResponseFuture` → `Result<MessageResult, SendError>`. So a message whose `result_type` is `Result<T, E>` needs `addr.send(msg).await.map_err(..)?.await.map_err(..)?` to reach `Result<T, E>`, then `?` to reach `T`.
- `ChildActor::start(label)` returns `Result<(Address<A>, JoinHandle<()>), _>`. Dropping the `JoinHandle` does not stop the actor; the actor lives while its `Address` is held.
- The whole refactor keeps the crate compiling after every task. `StartSession`/`Session` survive until Task 8; they are removed only in Task 9 once nothing references them.

## File Structure

**Created:**
- `src/llm/embedder.rs` — `Embedder` actor; owns `provider.embed`.
- `src/llm/synthesis.rs` — `SynthesisTask` actor, `SynthesisTarget`, `SourceDoc`, `Synthesize` message.
- `src/llm/template.rs` — `TemplateKind` + `load_template` (provider-specific resolution, hot-reload).
- `prompts/per_user.md`, `prompts/global.md` — default synthesis prompt templates.

**Modified:**
- `src/llm/provider.rs` — add `name()` to the `Provider` trait.
- `src/llm/provider/deepseek.rs` — implement `name()`.
- `src/llm/provider/mock.rs` — implement `name()`.
- `src/llm/config.rs` — add `prompts_dir`, `synthesis_context_max_chars`; rename `session_idle_timeout_secs` → `synthesis_idle_timeout_secs`.
- `src/llm.rs` — `Embedder` wiring, `Synthesize` handler, `TaskDied` internal message; `LlmService::new` returns `Result`; remove `StartSession`.
- `src/manager.rs` — adjust the `LlmService::new` call site.
- `src/memory/path.rs` — replace `derive_synthesis_path` with the date-named synthesis-path helpers `current_synthesis_path` / `latest_synthesis_path`.
- `src/memory/synthesizer.rs` — rewrite `process()` as a two-pass flow over `Synthesize`.
- `src/memory/manager.rs` — fix a test path assertion and a `LlmService::new` call site.

**Deleted:**
- `src/llm/session.rs` — the generic chat session actor (replaced by `SynthesisTask`).

---

## Task 1: Add `name()` to the `Provider` trait

**Files:**
- Modify: `src/llm/provider.rs:46-56` (trait), `src/llm/provider/deepseek.rs`, `src/llm/provider/mock.rs`
- Test: `src/llm/provider/mock.rs` (test module added at end)

- [ ] **Step 1: Write the failing test**

Append to `src/llm/provider/mock.rs` (after the `impl Provider for MockProvider` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_provider_reports_its_name() {
        let m = MockProvider::new();
        assert_eq!(m.name(), "mock");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p clawchorus --lib mock_provider_reports_its_name`
Expected: FAIL — compile error, `no method named name found for struct MockProvider`.

- [ ] **Step 3: Add `name()` to the trait**

In `src/llm/provider.rs`, add `name` as the first method of the `Provider` trait:

```rust
pub trait Provider: Send + Sync + 'static {
    /// Short provider identifier, e.g. `"deepseek"`. Used to resolve
    /// provider-specific prompt templates.
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
```

- [ ] **Step 4: Implement `name()` for `DeepSeekProvider`**

In `src/llm/provider/deepseek.rs`, inside `impl Provider for DeepSeekProvider`, add as the first method:

```rust
    fn name(&self) -> &str {
        "deepseek"
    }
```

- [ ] **Step 5: Implement `name()` for `MockProvider`**

In `src/llm/provider/mock.rs`, inside `impl Provider for MockProvider`, add as the first method:

```rust
    fn name(&self) -> &str {
        "mock"
    }
```

- [ ] **Step 6: Run tests and format**

Run: `cargo test -p clawchorus --lib && cargo fmt`
Expected: PASS — all existing tests plus `mock_provider_reports_its_name`.

- [ ] **Step 7: Commit**

```bash
git add src/llm/provider.rs src/llm/provider/deepseek.rs src/llm/provider/mock.rs
git commit -m "feat(llm): add name() to Provider trait"
```

---

## Task 2: Extend `LlmConfig` for synthesis

**Files:**
- Modify: `src/llm/config.rs`
- Modify: `src/llm.rs:86` (rename a field reference), `src/llm/provider/deepseek.rs:216` (test config field)
- Test: `src/llm/config.rs` (existing test module)

- [ ] **Step 1: Update the failing test**

In `src/llm/config.rs`, replace the `defaults_are_populated` test body and extend `partial_toml_uses_defaults_for_new_fields`:

```rust
    #[test]
    fn defaults_are_populated() {
        let c = LlmConfig::default();
        assert_eq!(c.synthesis_idle_timeout_secs, 300);
        assert_eq!(c.max_retries, 3);
        assert_eq!(c.request_timeout_secs, 30);
        assert_eq!(c.base_url, "https://api.deepseek.com");
        assert_eq!(c.prompts_dir, std::path::PathBuf::from("./prompts"));
        assert_eq!(c.synthesis_context_max_chars, 200_000);
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
        assert_eq!(c.synthesis_idle_timeout_secs, 300);
        assert_eq!(c.prompts_dir, std::path::PathBuf::from("./prompts"));
        assert_eq!(c.synthesis_context_max_chars, 200_000);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p clawchorus --lib defaults_are_populated`
Expected: FAIL — compile error, no field `synthesis_idle_timeout_secs` / `prompts_dir` / `synthesis_context_max_chars`.

- [ ] **Step 3: Update the struct, defaults, and `Default` impl**

In `src/llm/config.rs`, replace the `session_idle_timeout_secs` field and add the two new fields:

```rust
use std::path::PathBuf;

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
    /// Seconds a `SynthesisTask` will sit idle before self-terminating.
    #[serde(default = "default_synthesis_idle_timeout_secs")]
    pub synthesis_idle_timeout_secs: u64,
    /// Directory holding synthesis prompt templates.
    #[serde(default = "default_prompts_dir")]
    pub prompts_dir: PathBuf,
    /// Character budget for a `SynthesisTask`'s conversation history; once
    /// exceeded, the history is cleared and reseeded from the prior summary.
    #[serde(default = "default_synthesis_context_max_chars")]
    pub synthesis_context_max_chars: usize,
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

const fn default_synthesis_idle_timeout_secs() -> u64 {
    300
}

fn default_prompts_dir() -> PathBuf {
    PathBuf::from("./prompts")
}

const fn default_synthesis_context_max_chars() -> usize {
    200_000
}

const fn default_max_retries() -> u32 {
    3
}

const fn default_request_timeout_secs() -> u64 {
    30
}

fn default_base_url() -> String {
    "https://api.deepseek.com".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            model: "deepseek-chat".to_string(),
            embedding_model: "deepseek-embedding".to_string(),
            embedding_dim: None,
            synthesis_idle_timeout_secs: 300,
            prompts_dir: PathBuf::from("./prompts"),
            synthesis_context_max_chars: 200_000,
            max_retries: 3,
            request_timeout_secs: 30,
            base_url: "https://api.deepseek.com".to_string(),
        }
    }
}
```

- [ ] **Step 4: Fix the `session_idle_timeout_secs` references**

In `src/llm.rs:86`, change:

```rust
        let idle = Duration::from_secs(self.config.synthesis_idle_timeout_secs);
```

In `src/llm/provider/deepseek.rs` (the test config near line 216), change the field name `session_idle_timeout_secs: 60` to `synthesis_idle_timeout_secs: 60`.

In `src/llm.rs`, the test helper `cfg()` (around line 105) — change `session_idle_timeout_secs: 60` to `synthesis_idle_timeout_secs: 60`.

- [ ] **Step 5: Run tests and format**

Run: `cargo test -p clawchorus --lib && cargo fmt`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/llm/config.rs src/llm.rs src/llm/provider/deepseek.rs
git commit -m "feat(llm): add synthesis config fields, rename idle timeout"
```

---

## Task 3: Prompt template module + default templates

**Files:**
- Create: `src/llm/template.rs`
- Create: `prompts/per_user.md`, `prompts/global.md`
- Modify: `src/llm.rs` (register the module)

- [ ] **Step 1: Register the module**

In `src/llm.rs`, add alongside the other `pub mod` lines (after `pub mod config;`):

```rust
pub mod template;
```

- [ ] **Step 2: Write the failing test**

Create `src/llm/template.rs`:

```rust
//! Prompt template resolution and loading for synthesis.
//!
//! Templates are Markdown files on disk, read fresh on every use so prompt
//! changes take effect without recompiling or restarting.

use std::path::Path;

use super::error::LlmError;

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn provider_specific_template_wins_over_default() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "per_user.md", "default body");
        write(dir.path(), "deepseek/per_user.md", "deepseek body");

        let out = load_template(dir.path(), "deepseek", TemplateKind::PerUser).unwrap();
        assert_eq!(out, "deepseek body");
    }

    #[test]
    fn falls_back_to_default_template() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "global.md", "default global");

        let out = load_template(dir.path(), "deepseek", TemplateKind::Global).unwrap();
        assert_eq!(out, "default global");
    }

    #[test]
    fn missing_template_is_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_template(dir.path(), "deepseek", TemplateKind::PerUser).unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p clawchorus --lib template::`
Expected: FAIL — compile error, `TemplateKind` and `load_template` are undefined.

- [ ] **Step 4: Write the implementation**

In `src/llm/template.rs`, insert above the `#[cfg(test)]` module:

```rust
/// Which synthesis prompt a task uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    PerUser,
    Global,
}

impl TemplateKind {
    fn filename(self) -> &'static str {
        match self {
            TemplateKind::PerUser => "per_user.md",
            TemplateKind::Global => "global.md",
        }
    }
}

/// Load the synthesis prompt template for `(provider_name, kind)`.
///
/// Resolution: try `{prompts_dir}/{provider_name}/{kind}.md`; if that file is
/// absent, fall back to `{prompts_dir}/{kind}.md`. The file is read fresh on
/// every call so edits take effect without a restart. A missing template
/// returns [`LlmError::Config`].
pub fn load_template(
    prompts_dir: &Path,
    provider_name: &str,
    kind: TemplateKind,
) -> Result<String, LlmError> {
    let specific = prompts_dir.join(provider_name).join(kind.filename());
    let path = if specific.is_file() {
        specific
    } else {
        prompts_dir.join(kind.filename())
    };
    std::fs::read_to_string(&path)
        .map_err(|e| LlmError::Config(format!("prompt template {}: {}", path.display(), e)))
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p clawchorus --lib template::`
Expected: PASS — all three template tests.

- [ ] **Step 6: Create the default template files**

Create `prompts/per_user.md`:

```markdown
You are a memory synthesizer for a single user. You maintain a running,
deduplicated summary of everything that matters about this user's work,
decisions, and knowledge.

You will receive the current summary (if any) followed by new or changed
source documents. Fold the new information into the summary: add what is
new, update what changed, and drop nothing that still matters. Resolve
contradictions in favor of the most recent document.

Reply with the complete updated summary as Markdown, and nothing else.
```

Create `prompts/global.md`:

```markdown
You are a memory synthesizer for an organization. You maintain a running,
cross-user summary that captures shared knowledge, overlapping work, and
team-wide themes.

You will receive the current global summary (if any) followed by updated
per-user summaries. Fold them together: highlight what is common across
users, surface connections between their work, and keep individual detail
only where it matters organization-wide.

Reply with the complete updated global summary as Markdown, and nothing else.
```

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add src/llm.rs src/llm/template.rs prompts/per_user.md prompts/global.md
git commit -m "feat(llm): add prompt template module and default templates"
```

---

## Task 4: `Embedder` actor

**Files:**
- Create: `src/llm/embedder.rs`
- Modify: `src/llm.rs` (register the module)
- Test: `src/llm/embedder.rs` (test module)

- [ ] **Step 1: Register the module**

In `src/llm.rs`, add after `pub mod config;`:

```rust
pub mod embedder;
```

- [ ] **Step 2: Write the failing test**

Create `src/llm/embedder.rs`:

```rust
//! Embedder actor — long-lived child of `LlmService`. Owns `provider.embed`
//! so embedding is fully isolated from synthesis.

use std::sync::Arc;

use acktor::{Actor, Context, Handler, message::FutureMessageResult};
use tracing::trace;

use super::Embed;
use super::error::LlmError;
use super::provider::{self, Provider};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Embed;
    use crate::llm::provider::mock::MockProvider;

    #[tokio::test]
    async fn embedder_returns_mock_vectors() {
        let mock = Arc::new(MockProvider::new());
        mock.push_embed(Ok(MockProvider::canned_embed(3, 2, "mock-emb")));

        let (addr, _h) = Embedder::new(mock.clone(), 3).start("embedder-test").unwrap();

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
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p clawchorus --lib embedder::`
Expected: FAIL — compile error, `Embedder` is undefined.

- [ ] **Step 4: Write the implementation**

In `src/llm/embedder.rs`, insert above the `#[cfg(test)]` module:

```rust
pub struct Embedder {
    provider: Arc<dyn Provider>,
    max_retries: u32,
}

impl Embedder {
    pub fn new(provider: Arc<dyn Provider>, max_retries: u32) -> Self {
        Self {
            provider,
            max_retries,
        }
    }
}

impl Actor for Embedder {
    type Context = Context<Self>;
    type Error = LlmError;
}

impl Handler<Embed> for Embedder {
    type Result = FutureMessageResult<Embed>;

    async fn handle(&mut self, msg: Embed, _ctx: &mut Self::Context) -> FutureMessageResult<Embed> {
        trace!("Handle command {:?}", msg);
        let provider = self.provider.clone();
        let max_retries = self.max_retries;
        FutureMessageResult::new(async move {
            provider::retry(max_retries, || provider.embed(&msg.texts)).await
        })
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p clawchorus --lib embedder::`
Expected: PASS.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add src/llm.rs src/llm/embedder.rs
git commit -m "feat(llm): add Embedder actor"
```

---

## Task 5: `SynthesisTask` actor + `Synthesize` message

**Files:**
- Create: `src/llm/synthesis.rs`
- Modify: `src/llm.rs` (register the module)
- Test: `src/llm/synthesis.rs` (test module)

- [ ] **Step 1: Register the module**

In `src/llm.rs`, add after `pub mod embedder;`:

```rust
pub mod synthesis;
```

- [ ] **Step 2: Write the failing tests**

Create `src/llm/synthesis.rs`:

```rust
//! Synthesis — `SynthesisTask` actor plus its public message and value types.
//!
//! One long-lived `SynthesisTask` exists per `SynthesisTarget`. It owns the
//! conversation context for that target so successive cool-down cycles refine
//! a synthesis instead of rebuilding it. It hot-reloads its prompt template,
//! reseeds an empty history from a caller-supplied prior summary, and clears
//! its history once it grows past a character budget.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use acktor::{
    Actor, ActorContext, Handler, Message, Signal,
    cron::{CronActor, CronContext},
    message::FutureMessageResult,
};
use tokio::time::Instant;
use tracing::trace;

use super::error::LlmError;
use super::provider::{ChatMessage, Provider, Role, retry};
use super::template::{TemplateKind, load_template};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::ChatResponse;
    use crate::llm::provider::mock::MockProvider;

    /// Build a temp prompts dir holding a `per_user.md` template.
    fn prompts_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("per_user.md"), "SYSTEM PROMPT").unwrap();
        dir
    }

    fn task(mock: Arc<MockProvider>, dir: &tempfile::TempDir, max_chars: usize) -> SynthesisTask {
        SynthesisTask::new(
            mock,
            TemplateKind::PerUser,
            dir.path().to_path_buf(),
            Duration::from_secs(60),
            3,
            max_chars,
        )
    }

    fn doc(name: &str, content: &str) -> SourceDoc {
        SourceDoc {
            name: name.into(),
            content: content.into(),
        }
    }

    #[tokio::test]
    async fn reseeds_empty_history_from_prior_summary() {
        let mock = Arc::new(MockProvider::new());
        let dir = prompts_dir();
        let (addr, _h) = task(mock.clone(), &dir, 1_000_000).start("synth-test").unwrap();

        addr.send(Synthesize {
            target: SynthesisTarget::User("alice".into()),
            prior_summary: Some("PRIOR-SUMMARY".into()),
            sources: vec![doc("a.md", "new content")],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let call = mock.last_chat_call().unwrap();
        // system + reseed turn + sources turn
        assert_eq!(call[0].role, Role::System);
        assert_eq!(call[0].content, "SYSTEM PROMPT");
        assert!(call.iter().any(|m| m.content.contains("PRIOR-SUMMARY")));
        assert!(call.iter().any(|m| m.content.contains("new content")));
    }

    #[tokio::test]
    async fn accumulates_history_across_calls() {
        let mock = Arc::new(MockProvider::new());
        mock.push_chat(Ok(ChatResponse {
            model: "m".into(),
            content: "reply-2".into(),
        }));
        mock.push_chat(Ok(ChatResponse {
            model: "m".into(),
            content: "reply-1".into(),
        }));
        let dir = prompts_dir();
        let (addr, _h) = task(mock.clone(), &dir, 1_000_000).start("synth-test").unwrap();

        for (n, text) in [(1, "cycle-1"), (2, "cycle-2")] {
            addr.send(Synthesize {
                target: SynthesisTarget::User("alice".into()),
                prior_summary: None,
                sources: vec![doc("a.md", text)],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
            let _ = n;
        }

        let call = mock.last_chat_call().unwrap();
        // Second call still carries the first cycle's turns.
        assert!(call.iter().any(|m| m.content.contains("cycle-1")));
        assert!(call.iter().any(|m| m.content == "reply-1"));
        assert!(call.iter().any(|m| m.content.contains("cycle-2")));
    }

    #[tokio::test]
    async fn resets_context_when_over_budget() {
        let mock = Arc::new(MockProvider::new());
        let dir = prompts_dir();
        // Budget of 1 char guarantees a reset after the first call.
        let (addr, _h) = task(mock.clone(), &dir, 1).start("synth-test").unwrap();

        addr.send(Synthesize {
            target: SynthesisTarget::User("alice".into()),
            prior_summary: None,
            sources: vec![doc("a.md", "first")],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(Synthesize {
            target: SynthesisTarget::User("alice".into()),
            prior_summary: Some("RESEED".into()),
            sources: vec![doc("b.md", "second")],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let call = mock.last_chat_call().unwrap();
        // History was cleared after call 1, so call 2 reseeds and the
        // first cycle's content is gone.
        assert!(call.iter().any(|m| m.content.contains("RESEED")));
        assert!(!call.iter().any(|m| m.content.contains("first")));
        assert!(call.iter().any(|m| m.content.contains("second")));
    }

    #[tokio::test]
    async fn missing_template_returns_config_error() {
        let mock = Arc::new(MockProvider::new());
        let empty = tempfile::tempdir().unwrap();
        let (addr, _h) = task(mock, &empty, 1_000_000).start("synth-test").unwrap();

        let err = addr
            .send(Synthesize {
                target: SynthesisTarget::User("alice".into()),
                prior_summary: None,
                sources: vec![doc("a.md", "x")],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
    }

    #[tokio::test]
    async fn idle_timeout_terminates_task() {
        let mock = Arc::new(MockProvider::new());
        let dir = prompts_dir();
        let t = SynthesisTask::new(
            mock,
            TemplateKind::PerUser,
            dir.path().to_path_buf(),
            Duration::from_millis(100),
            3,
            1_000_000,
        );
        let (_addr, handle) = t.start("synth-idle").unwrap();
        let res = tokio::time::timeout(Duration::from_millis(800), handle).await;
        assert!(res.is_ok(), "task should self-terminate on idle");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p clawchorus --lib synthesis::`
Expected: FAIL — compile error, `SynthesisTask`, `Synthesize`, `SynthesisTarget`, `SourceDoc` undefined.

- [ ] **Step 4: Write the value types and message**

In `src/llm/synthesis.rs`, insert above the `#[cfg(test)]` module:

```rust
/// Which synthesis a `Synthesize` request targets. Identifies both the
/// long-lived task to route to and the prompt template kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SynthesisTarget {
    /// Per-user synthesis; the `String` is the username.
    User(String),
    /// Cross-user synthesis.
    Global,
}

impl SynthesisTarget {
    /// Prompt template kind for this target.
    pub fn template_kind(&self) -> TemplateKind {
        match self {
            SynthesisTarget::User(_) => TemplateKind::PerUser,
            SynthesisTarget::Global => TemplateKind::Global,
        }
    }

    /// Stable actor label for this target.
    pub fn label(&self) -> String {
        match self {
            SynthesisTarget::User(u) => format!("synth-user-{u}"),
            SynthesisTarget::Global => "synth-global".to_string(),
        }
    }
}

/// One source document fed into a synthesis.
#[derive(Debug, Clone)]
pub struct SourceDoc {
    /// A label for the document, e.g. its relative path.
    pub name: String,
    pub content: String,
}

/// Synthesize `sources` into the target's running summary. `prior_summary`
/// is the current on-disk summary; it seeds a task whose context is empty
/// (cold start, after a restart, or after a context reset).
#[derive(Debug, Message)]
#[result_type(Result<String, LlmError>)]
pub struct Synthesize {
    pub target: SynthesisTarget,
    pub prior_summary: Option<String>,
    pub sources: Vec<SourceDoc>,
}

fn render_sources(sources: &[SourceDoc]) -> String {
    let mut out = String::from("New or changed documents to fold into the synthesis:\n\n");
    for doc in sources {
        out.push_str(&format!("## {}\n\n{}\n\n", doc.name, doc.content));
    }
    out
}
```

- [ ] **Step 5: Write the `SynthesisTask` actor**

In `src/llm/synthesis.rs`, append after `render_sources` (still above the test module):

```rust
/// Long-lived per-target synthesis actor. Owns the conversation history for
/// its target and self-terminates on idle via `CronActor`.
pub struct SynthesisTask {
    provider: Arc<dyn Provider>,
    kind: TemplateKind,
    prompts_dir: PathBuf,
    history: Vec<ChatMessage>,
    idle_timeout: Duration,
    last_activity: Instant,
    max_retries: u32,
    context_max_chars: usize,
}

impl SynthesisTask {
    pub fn new(
        provider: Arc<dyn Provider>,
        kind: TemplateKind,
        prompts_dir: PathBuf,
        idle_timeout: Duration,
        max_retries: u32,
        context_max_chars: usize,
    ) -> Self {
        Self {
            provider,
            kind,
            prompts_dir,
            history: Vec::new(),
            idle_timeout,
            last_activity: Instant::now(),
            max_retries,
            context_max_chars,
        }
    }
}

impl Actor for SynthesisTask {
    type Context = CronContext<Self>;
    type Error = LlmError;
}

impl CronActor for SynthesisTask {
    async fn task(&mut self, ctx: &mut Self::Context) -> Result<Duration, LlmError> {
        let elapsed = self.last_activity.elapsed();
        if elapsed >= self.idle_timeout {
            trace!("SynthesisTask idle for {:?}, terminating", elapsed);
            let _ = ctx.address().do_send(Signal::Terminate).await;
            return Ok(Duration::from_secs(3600));
        }
        let remaining = self.idle_timeout.saturating_sub(elapsed);
        Ok(remaining.max(Duration::from_millis(50)))
    }
}

impl Handler<Synthesize> for SynthesisTask {
    type Result = FutureMessageResult<Synthesize>;

    async fn handle(
        &mut self,
        msg: Synthesize,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<Synthesize> {
        trace!("Handle command {:?}", msg);
        self.last_activity = Instant::now();

        // Hot-reload the prompt template every call.
        let system = match load_template(&self.prompts_dir, self.provider.name(), self.kind) {
            Ok(t) => t,
            Err(e) => return FutureMessageResult::new(async move { Err(e) }),
        };

        // Snapshot history length so a failed call can be rolled back cleanly.
        let restore_len = self.history.len();

        // Reseed an empty history from the prior summary.
        if self.history.is_empty() {
            if let Some(summary) = msg.prior_summary {
                self.history.push(ChatMessage {
                    role: Role::User,
                    content: format!("Current summary so far:\n\n{summary}"),
                });
            }
        }

        // Append the changed sources as a user turn.
        self.history.push(ChatMessage {
            role: Role::User,
            content: render_sources(&msg.sources),
        });

        // System message is recomputed each call, never stored in history.
        let mut messages = Vec::with_capacity(self.history.len() + 1);
        messages.push(ChatMessage {
            role: Role::System,
            content: system,
        });
        messages.extend(self.history.iter().cloned());

        let provider = self.provider.clone();
        let result = retry(self.max_retries, || provider.chat(&messages)).await;

        let reply = match result {
            Ok(resp) => resp.content,
            Err(e) => {
                self.history.truncate(restore_len);
                return FutureMessageResult::new(async move { Err(e) });
            }
        };

        self.history.push(ChatMessage {
            role: Role::Assistant,
            content: reply.clone(),
        });
        self.last_activity = Instant::now();

        // Reset context once it grows past the budget; the next call reseeds.
        let total: usize = self.history.iter().map(|m| m.content.len()).sum();
        if total > self.context_max_chars {
            trace!("synthesis context {} chars over budget, clearing", total);
            self.history.clear();
        }

        FutureMessageResult::new(async move { Ok(reply) })
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p clawchorus --lib synthesis::`
Expected: PASS — all five synthesis tests.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add src/llm.rs src/llm/synthesis.rs
git commit -m "feat(llm): add SynthesisTask actor and Synthesize message"
```

---

## Task 6: Wire `Embedder` + `Synthesize` into `LlmService`

This task rewrites `src/llm.rs`. `StartSession` and the `Session` actor are intentionally **kept** so the crate keeps compiling — they are removed in Task 9.

**Files:**
- Modify: `src/llm.rs`
- Modify: `src/manager.rs:56-59`, `src/memory/manager.rs:243-247`, `src/memory/synthesizer.rs:300-306` (the `LlmService::new` call sites)

- [ ] **Step 1: Write the failing test**

In `src/llm.rs`, add to the `tests` module a new test:

```rust
    #[tokio::test]
    async fn synthesize_spawns_task_and_returns_reply() {
        let mock = Arc::new(MockProvider::new());
        mock.push_chat(Ok(ChatResponse {
            model: "mock-chat".into(),
            content: "synth-reply".into(),
        }));

        let svc = LlmService::new(cfg(), mock.clone()).unwrap();
        let (addr, _h) = svc.start("llm-test").unwrap();

        let reply = addr
            .send(Synthesize {
                target: crate::llm::SynthesisTarget::User("alice".into()),
                prior_summary: None,
                sources: vec![crate::llm::SourceDoc {
                    name: "alice/a/x.md".into(),
                    content: "hello".into(),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(reply, "synth-reply");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p clawchorus --lib synthesize_spawns_task_and_returns_reply`
Expected: FAIL — compile error, no `Synthesize` / `SynthesisTarget` in `crate::llm`, and `LlmService::new` does not return `Result`.

- [ ] **Step 3: Rewrite the module head of `src/llm.rs`**

Replace lines 1-55 of `src/llm.rs` (the doc comment, imports, module decls, type definitions, `LlmService` struct, and `impl LlmService`) with:

```rust
//! LLM Service Actor — the single entry point for embedding and synthesis.
//!
//! Sibling of Memory Manager, supervised by the top-level Manager. It spawns
//! an `Embedder` child and one long-lived `SynthesisTask` child per target.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acktor::{
    Actor, ActorContext, Address, Context, Handler, Message, message::FutureMessageResult,
};
use tracing::trace;

use crate::llm::config::LlmConfig;
use crate::llm::embedder::Embedder;
use crate::llm::session::Session;
use crate::llm::synthesis::SynthesisTask;

mod error;
pub use crate::llm::error::LlmError;

pub mod config;
pub mod embedder;
pub mod session;
pub mod synthesis;
pub mod template;

pub mod provider;
pub use provider::{Provider, build_provider};

pub use synthesis::{SourceDoc, Synthesize, SynthesisTarget};

/// A single embedding vector.
#[derive(Debug)]
pub struct Embedding(pub Vec<f32>);

/// Result of an embedding request.
#[derive(Debug)]
pub struct EmbedResult {
    pub model: String,
    pub embeddings: Vec<Embedding>,
}

/// Embed a batch of text strings, returning one [`Embedding`] per input.
#[derive(Debug, Message)]
#[result_type(Result<EmbedResult, LlmError>)]
pub struct Embed {
    pub texts: Vec<String>,
}

/// Open a new conversation session. Reply is the spawned `Session` actor's address.
#[derive(Debug, Message)]
#[result_type(Result<Address<Session>, LlmError>)]
pub struct StartSession;

/// Internal: a `SynthesisTask` has terminated (idle), so drop its map entry.
#[derive(Debug, Message)]
#[result_type(())]
struct TaskDied {
    target: SynthesisTarget,
}

pub struct LlmService {
    config: LlmConfig,
    provider: Arc<dyn Provider>,
    embedder: Address<Embedder>,
    tasks: HashMap<SynthesisTarget, Address<SynthesisTask>>,
}

impl LlmService {
    /// Build the service, spawning its long-lived `Embedder` child.
    pub fn new(config: LlmConfig, provider: Arc<dyn Provider>) -> Result<Self, LlmError> {
        let (embedder, _handle) = Embedder::new(provider.clone(), config.max_retries)
            .start("embedder")
            .map_err(|e| LlmError::Actor(e.to_string()))?;
        Ok(Self {
            config,
            provider,
            embedder,
            tasks: HashMap::new(),
        })
    }
}
```

- [ ] **Step 4: Replace the handlers in `src/llm.rs`**

Replace the `Handler<Embed>` and `Handler<StartSession>` impl blocks (the old lines 62-95) with:

```rust
impl Handler<Embed> for LlmService {
    type Result = FutureMessageResult<Embed>;

    async fn handle(&mut self, msg: Embed, _ctx: &mut Self::Context) -> FutureMessageResult<Embed> {
        trace!("Handle command {:?}", msg);
        let embedder = self.embedder.clone();
        FutureMessageResult::new(async move {
            embedder
                .send(msg)
                .await
                .map_err(|e| LlmError::Actor(e.to_string()))?
                .await
                .map_err(|e| LlmError::Actor(e.to_string()))?
        })
    }
}

impl Handler<Synthesize> for LlmService {
    type Result = FutureMessageResult<Synthesize>;

    async fn handle(
        &mut self,
        msg: Synthesize,
        ctx: &mut Self::Context,
    ) -> FutureMessageResult<Synthesize> {
        trace!("Handle command {:?}", msg);

        // Get-or-spawn the long-lived task for this target.
        let task = match self.tasks.get(&msg.target) {
            Some(addr) => addr.clone(),
            None => {
                let target = msg.target.clone();
                let label = target.label();
                let new_task = SynthesisTask::new(
                    self.provider.clone(),
                    target.template_kind(),
                    self.config.prompts_dir.clone(),
                    Duration::from_secs(self.config.synthesis_idle_timeout_secs),
                    self.config.max_retries,
                    self.config.synthesis_context_max_chars,
                );
                match new_task.start(&label) {
                    Ok((addr, handle)) => {
                        // When the task self-terminates on idle, tell ourselves
                        // so the stale map entry is dropped.
                        let self_addr = ctx.address().clone();
                        let dead = target.clone();
                        tokio::spawn(async move {
                            let _ = handle.await;
                            let _ = self_addr.do_send(TaskDied { target: dead }).await;
                        });
                        self.tasks.insert(target, addr.clone());
                        addr
                    }
                    Err(e) => {
                        let err = LlmError::Actor(e.to_string());
                        return FutureMessageResult::new(async move { Err(err) });
                    }
                }
            }
        };

        FutureMessageResult::new(async move {
            task.send(msg)
                .await
                .map_err(|e| LlmError::Actor(e.to_string()))?
                .await
                .map_err(|e| LlmError::Actor(e.to_string()))?
        })
    }
}

impl Handler<TaskDied> for LlmService {
    type Result = ();

    async fn handle(&mut self, msg: TaskDied, _ctx: &mut Self::Context) {
        trace!("Handle command {:?}", msg);
        self.tasks.remove(&msg.target);
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
        let idle = Duration::from_secs(self.config.synthesis_idle_timeout_secs);
        let max_retries = self.config.max_retries;
        FutureMessageResult::new(async move {
            let (addr, _handle) = Session::new(provider, model, idle, max_retries)
                .start("session")
                .map_err(|e| LlmError::Actor(e.to_string()))?;
            Ok(addr)
        })
    }
}
```

Note: the `Actor for LlmService` impl block (old lines 57-60) stays unchanged between the `impl LlmService` block and the handlers.

- [ ] **Step 5: Fix the `LlmService::new` call sites**

`src/manager.rs:56-59` — change the closure body to return the `Result` directly:

```rust
        let (llm_addr, llm_handle) = LlmService::create("llm-service", |child_ctx| {
            child_ctx.set_supervisor(Some(ctx.address().into()));
            LlmService::new(llm, provider)
        })?;
```

`src/memory/manager.rs` — in the `test_llm` helper (around line 243-247), add `.unwrap()`:

```rust
    fn test_llm() -> Address<LlmService> {
        let provider = std::sync::Arc::new(crate::llm::provider::mock::MockProvider::new());
        let llm = LlmService::new(Default::default(), provider).unwrap();
        let (addr, _handle) = llm.start("llm-test").unwrap();
        addr
    }
```

`src/memory/synthesizer.rs` — in the `boot` test helper (around line 303), add `.unwrap()`:

```rust
        let (llm, h3) = LlmService::new(Default::default(), provider)
            .unwrap()
            .start("l")
            .unwrap();
```

`src/llm.rs` tests — the two existing `LlmService::new(cfg(), mock.clone())` lines (in `embed_returns_mock_vectors` and `start_session_returns_working_address`) become `LlmService::new(cfg(), mock.clone()).unwrap()`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p clawchorus --lib && cargo fmt`
Expected: PASS — including `synthesize_spawns_task_and_returns_reply`. (`start_session_returns_working_address` still passes; removed in Task 9.)

- [ ] **Step 7: Commit**

```bash
git add src/llm.rs src/manager.rs src/memory/manager.rs src/memory/synthesizer.rs
git commit -m "feat(llm): wire Embedder and Synthesize into LlmService"
```

---

## Task 7: Synthesis path helpers

`derive_synthesis_path` is kept here so the crate compiles; it is removed in Task 9 after the Synthesizer stops using it.

Synthesized output uses **date-named files** under the target's `_synthesized/` folder (see `memory-manager-design.md` → "Synthesized output files"):

- Filename for a synthesis run on UTC date `D` is `{D}.md` (e.g. `2026-05-20.md`).
- The "current" file (used both as the write target and as `prior_summary`) is the lexicographically greatest existing `{date}[-{n}].md` in the folder, or a fresh `{today}.md` if none exists.
- A size cap `synthesis.max_file_bytes` (default 1 MiB) rolls the writer over to `{D}-1.md`, `{D}-2.md`, … when the would-be reused file is already at or above the cap.

There is no `memory_type` segment anywhere in the synthesized path — folding happens across the entire user (or the entire global set).

**Files:**
- Modify: `src/memory/path.rs`
- Test: `src/memory/path.rs` (test module)

- [ ] **Step 1: Write the failing test**

In `src/memory/path.rs`, add to the `tests` module:

```rust
    #[test]
    fn per_user_synthesis_path_uses_date_name() {
        // Empty folder: fresh `{today}.md`, no suffix.
        let p = current_synthesis_path(
            std::path::Path::new("/nonexistent-memory-dir"),
            &SynthesisTarget::User("alice".into()),
            "2026-05-20",
            1024 * 1024,
        );
        assert_eq!(p, "alice/_synthesized/2026-05-20.md");
    }

    #[test]
    fn global_synthesis_path_uses_date_name() {
        let p = current_synthesis_path(
            std::path::Path::new("/nonexistent-memory-dir"),
            &SynthesisTarget::Global,
            "2026-05-20",
            1024 * 1024,
        );
        assert_eq!(p, "_synthesized/2026-05-20.md");
    }
```

`SynthesisTarget` is the type from `crate::llm` introduced in Task 5. The "today" string is passed in as a parameter so this test does not depend on wall-clock time; callers in production will source it from the `Clock` trait described in the memory-manager plan (clock injection is **not** introduced by this task — leave a TODO in the call sites until that lands).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p clawchorus --lib path::`
Expected: FAIL — compile error, `current_synthesis_path` / `latest_synthesis_path` undefined.

- [ ] **Step 3: Add the helpers**

In `src/memory/path.rs`, add after `derive_synthesis_path`:

```rust
use std::path::Path;

use crate::llm::SynthesisTarget;

/// Folder (relative to `memory_dir`) that holds synthesized output for `target`.
///
/// - `SynthesisTarget::User(u)` → `"{u}/_synthesized"`
/// - `SynthesisTarget::Global` → `"_synthesized"`
fn synthesis_folder(target: &SynthesisTarget) -> String {
    match target {
        SynthesisTarget::User(u) => format!("{}/_synthesized", u),
        SynthesisTarget::Global => "_synthesized".to_string(),
    }
}

/// Return the path of the most recent existing synthesized file for `target`,
/// or `None` if the folder is empty / does not exist.
///
/// "Most recent" = lexicographically greatest filename of the form
/// `{date}[-{n}].md` under `{memory_dir}/{folder}`. The lexicographic order on
/// the `YYYY-MM-DD[-{n}]` stem matches the desired write order because dates
/// are zero-padded ISO-8601 and the optional `-{n}` suffix sorts after the
/// bare date.
pub fn latest_synthesis_path(memory_dir: &Path, target: &SynthesisTarget) -> Option<String> {
    let folder = synthesis_folder(target);
    let abs = memory_dir.join(&folder);
    let entries = std::fs::read_dir(&abs).ok()?;
    let mut best: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") {
            continue;
        }
        if best.as_deref().map_or(true, |b| name.as_str() > b) {
            best = Some(name);
        }
    }
    best.map(|name| format!("{}/{}", folder, name))
}

/// Return the path the next synthesis run should write to for `target`.
///
/// Rules (see `memory-manager-design.md` → "Synthesized output files"):
///
/// 1. If no synthesized file exists yet under the target folder, write
///    `{folder}/{today}.md`.
/// 2. Otherwise inspect the lexicographically greatest existing file. If its
///    on-disk size is **below** `max_file_bytes`, reuse it (the writer will
///    overwrite — accumulation comes from new dated files, not from
///    concatenation within one file).
/// 3. If that file is at or above the cap, roll over: take its date stem and
///    return the next free `{stem}-{n}.md` (starting at `n=1` and
///    incrementing past any existing suffixes).
///
/// `today` is the UTC date in `YYYY-MM-DD` form, supplied by the caller. The
/// helper deliberately does not read a clock so it is trivially testable; the
/// Synthesizer will obtain it from the `Clock` trait introduced by the
/// memory-manager plan.
pub fn current_synthesis_path(
    memory_dir: &Path,
    target: &SynthesisTarget,
    today: &str,
    max_file_bytes: u64,
) -> String {
    let folder = synthesis_folder(target);

    let Some(latest) = latest_synthesis_path(memory_dir, target) else {
        return format!("{}/{}.md", folder, today);
    };

    let abs = memory_dir.join(&latest);
    let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
    if size < max_file_bytes {
        return latest;
    }

    // Roll over. Strip the trailing ".md" and any existing "-{n}" suffix to
    // recover the date stem; then probe `-1`, `-2`, … until we find a free
    // slot.
    let file_name = latest.rsplit('/').next().unwrap_or(&latest);
    let stem = file_name.strip_suffix(".md").unwrap_or(file_name);
    let date_stem = match stem.rsplit_once('-') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => head.to_string(),
        _ => stem.to_string(),
    };
    let mut n: u32 = 1;
    loop {
        let candidate = format!("{}/{}-{}.md", folder, date_stem, n);
        if !memory_dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}
```

Note: the legacy `per_user_synthesis_path` / `global_synthesis_path` flat helpers from earlier drafts of this plan are **not** introduced — they hard-coded `summary.md` and have been superseded by `current_synthesis_path` / `latest_synthesis_path`. The Synthesizer (Task 8) calls the new helpers directly.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p clawchorus --lib path:: && cargo fmt`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/path.rs
git commit -m "feat(memory): add stable synthesis path helpers"
```

---

## Task 8: Rewrite the Synthesizer as a two-pass flow

**Files:**
- Modify: `src/memory/synthesizer.rs`
- Modify: `src/memory/manager.rs` (test path assertion around line 388-400)

- [ ] **Step 1: Update the Synthesizer's own test**

In `src/memory/synthesizer.rs`, replace the `synthesizer_writes_synthesis_after_cooldown` test's assertion block (the `target_dir` / `entries` part, old lines 337-350) with a stable-path check via the new helper, and delete the two `common_username` tests entirely:

```rust
        tokio::time::sleep(Duration::from_millis(200)).await;

        // The Synthesizer should have written exactly one per-user
        // synthesized file under `alice/_synthesized/`. We don't hard-code the
        // filename — it is `{utc-today}.md` — so we look it up via the
        // helper introduced in Task 7.
        let latest = crate::memory::path::latest_synthesis_path(
            dir.path(),
            &crate::llm::SynthesisTarget::User("alice".into()),
        );
        let rel = latest.expect("expected a per-user synthesized file under alice/_synthesized");
        let summary = dir.path().join(&rel);
        assert!(
            summary.is_file(),
            "expected per-user summary at {:?}",
            summary
        );
        assert!(
            rel.starts_with("alice/_synthesized/") && rel.ends_with(".md"),
            "unexpected synthesized path: {}",
            rel
        );
```

(Delete `common_username_detects_single_owner` and `common_username_returns_none_for_mixed_owners` — the function they test is being removed.)

- [ ] **Step 2: Update the MemoryManager integration test**

In `src/memory/manager.rs`, in `write_emits_synthesis_after_cooldown`, replace the `synth_dir` / `entries` assertion block (old lines 388-400) with the analogous helper-based check:

```rust
        let latest = crate::memory::path::latest_synthesis_path(
            dir.path(),
            &crate::llm::SynthesisTarget::User("alice".into()),
        );
        let rel = latest.expect("expected a per-user synthesized file under alice/_synthesized");
        let summary = dir.path().join(&rel);
        assert!(
            summary.is_file(),
            "expected per-user summary at {:?}",
            summary
        );
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p clawchorus --lib synthesizer_writes_synthesis_after_cooldown`
Expected: FAIL — the old timestamped file is written under `.../long_term/`, so no `alice/_synthesized/{date}.md` exists yet. (`common_username` tests no longer compile if not deleted — ensure they are deleted.)

- [ ] **Step 4: Rewrite the module head**

Replace lines 1-27 of `src/memory/synthesizer.rs` (doc comment through the `use` block) with:

```rust
//! Synthesizer Actor — long-lived child of MemoryManager.
//!
//! Receives fire-and-forget `FileChanged` notifications, batches them with a
//! cool-down timer, then runs a two-pass synthesis. The per-user pass folds
//! each affected user's changed files into that user's running summary; the
//! global pass folds the changed per-user summaries into a cross-user
//! summary. Synthesis itself is delegated to LLM Service via `Synthesize`;
//! this actor only reads sources, writes results, and indexes them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use acktor::{Actor, ActorContext, Address, Context, Handler, Message};
use tokio::time::Instant;
use tracing::{info, trace, warn};

use crate::llm::{Embed, EmbedResult, LlmService, SourceDoc, Synthesize, SynthesisTarget};
use crate::memory::{
    chunking::chunk_text,
    error::MemoryError,
    index::Index,
    messages::{Chunk, EnsureVecReady, FileChanged, IndexInsert, StorageRead, StorageWrite},
    path::{current_synthesis_path, latest_synthesis_path},
    storage::Storage,
};
```

- [ ] **Step 5: Replace the `impl Synthesizer` body**

Replace the entire `impl Synthesizer { ... }` block that contains `process()` and the free functions `build_synthesis_prompt`, `common_username`, `synthesis_filename` (old lines 43-246) with the following. Keep the `CooldownTick` struct (old lines 28-30) as it is, and extend the `Synthesizer` struct (old lines 32-41) with three new fields used by the date-named synthesis paths:

```rust
pub struct Synthesizer {
    // ...existing fields (storage, index, llm, cooldown, chunk_size,
    //                    chunk_overlap, pending, last_event)...

    /// Root memory directory; needed to resolve absolute paths when looking
    /// up the latest synthesized file and the rollover index.
    memory_dir: std::path::PathBuf,
    /// Size cap (bytes) that triggers rollover to `{date}-{n}.md`.
    /// Sourced from `synthesis.max_file_bytes` in the memory config.
    max_file_bytes: u64,
    /// UTC-date provider, so tests can pin the date.
    /// **Requires** the `Clock` trait introduced by the memory-manager plan;
    /// until that lands, leave a `TODO(clock)` and inject a fixed-clock
    /// implementation in tests.
    clock: std::sync::Arc<dyn crate::memory::Clock>,
}
```

```rust
impl Synthesizer {
    pub fn new(
        storage: Address<Storage>,
        index: Address<Index>,
        llm: Address<LlmService>,
        cooldown_secs: u64,
        chunk_size: usize,
        chunk_overlap: usize,
        memory_dir: std::path::PathBuf,
        max_file_bytes: u64,
        clock: std::sync::Arc<dyn crate::memory::Clock>,
    ) -> Self {
        Self {
            storage,
            index,
            llm,
            cooldown: Duration::from_secs(cooldown_secs),
            chunk_size,
            chunk_overlap,
            pending: BTreeSet::new(),
            last_event: None,
            memory_dir,
            max_file_bytes,
            clock,
        }
    }

    /// Run the two-pass synthesis over the pending set.
    async fn process(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let paths: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
        info!("Synthesizer: processing {} pending paths", paths.len());

        // Group the changed paths by owning user.
        let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in paths {
            match path.split('/').next() {
                Some(u) if !u.is_empty() && u != "_synthesized" => {
                    by_user.entry(u.to_string()).or_default().push(path);
                }
                _ => {}
            }
        }

        // Per-user pass.
        let mut changed_users: Vec<String> = Vec::new();
        for (user, changed) in &by_user {
            match self.synthesize_user(user, changed).await {
                Ok(true) => changed_users.push(user.clone()),
                Ok(false) => {}
                Err(e) => warn!("Synthesizer: per-user synthesis for {} failed: {}", user, e),
            }
        }

        // Global pass.
        if !changed_users.is_empty() {
            if let Err(e) = self.synthesize_global(&changed_users).await {
                warn!("Synthesizer: global synthesis failed: {}", e);
            }
        }
    }

    /// Re-synthesize one user from their changed files. Returns `true` if a
    /// summary was written, `false` if there was nothing to synthesize.
    async fn synthesize_user(
        &self,
        user: &str,
        changed: &[String],
    ) -> Result<bool, MemoryError> {
        let mut sources = Vec::new();
        for path in changed {
            match self.storage_read(path).await? {
                Some(content) => sources.push(SourceDoc {
                    name: path.clone(),
                    content,
                }),
                None => trace!("Synthesizer: source {} vanished, skipping", path),
            }
        }
        if sources.is_empty() {
            return Ok(false);
        }

        let target = SynthesisTarget::User(user.to_string());
        let prior_path = latest_synthesis_path(&self.memory_dir, &target);
        let prior_summary = match &prior_path {
            Some(p) => self.storage_read(p).await?,
            None => None,
        };
        let synthesis = self
            .request_synthesis(target.clone(), prior_summary, sources)
            .await?;
        // Write target: reuse current file if it is still under the cap,
        // otherwise roll over to the next `-{n}` suffix.
        let today = self.clock.today_utc();
        let summary_path =
            current_synthesis_path(&self.memory_dir, &target, &today, self.max_file_bytes);
        self.write_synthesis(&summary_path, &synthesis).await?;
        info!("Synthesizer: per-user synthesis written to {}", summary_path);
        Ok(true)
    }

    /// Synthesize the cross-user summary from the changed per-user summaries.
    async fn synthesize_global(&self, changed_users: &[String]) -> Result<(), MemoryError> {
        let mut sources = Vec::new();
        for user in changed_users {
            let target = SynthesisTarget::User(user.clone());
            let Some(path) = latest_synthesis_path(&self.memory_dir, &target) else {
                continue;
            };
            if let Some(content) = self.storage_read(&path).await? {
                sources.push(SourceDoc { name: path, content });
            }
        }
        if sources.is_empty() {
            return Ok(());
        }

        let target = SynthesisTarget::Global;
        let prior_path = latest_synthesis_path(&self.memory_dir, &target);
        let prior_summary = match &prior_path {
            Some(p) => self.storage_read(p).await?,
            None => None,
        };
        let synthesis = self
            .request_synthesis(target.clone(), prior_summary, sources)
            .await?;
        let today = self.clock.today_utc();
        let summary_path =
            current_synthesis_path(&self.memory_dir, &target, &today, self.max_file_bytes);
        self.write_synthesis(&summary_path, &synthesis).await?;
        info!("Synthesizer: global synthesis written to {}", summary_path);
        Ok(())
    }

    /// Chunk, embed, and write a synthesized document to Storage and Index.
    async fn write_synthesis(&self, path: &str, content: &str) -> Result<(), MemoryError> {
        self.storage_write(path, content).await?;

        let text_chunks = chunk_text(content, self.chunk_size, self.chunk_overlap);
        if text_chunks.is_empty() {
            return Ok(());
        }
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embed_result = self.embed(texts).await?;
        let chunks: Vec<Chunk> = text_chunks
            .into_iter()
            .zip(embed_result.embeddings)
            .map(|(tc, emb)| Chunk {
                text: tc.text,
                start_line: tc.start_line,
                end_line: tc.end_line,
                embedding: emb,
            })
            .collect();
        self.index_insert(path, content, chunks, embed_result.model)
            .await
    }

    async fn storage_read(&self, path: &str) -> Result<Option<String>, MemoryError> {
        let fut = self
            .storage
            .send(StorageRead {
                path: path.to_string(),
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        let res = fut.await.map_err(|e| MemoryError::Actor(e.to_string()))?;
        Ok(res?)
    }

    async fn storage_write(&self, path: &str, content: &str) -> Result<(), MemoryError> {
        let fut = self
            .storage
            .send(StorageWrite {
                path: path.to_string(),
                content: content.to_string(),
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        fut.await.map_err(|e| MemoryError::Actor(e.to_string()))??;
        Ok(())
    }

    async fn embed(&self, texts: Vec<String>) -> Result<EmbedResult, MemoryError> {
        let fut = self
            .llm
            .send(Embed { texts })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        let res = fut.await.map_err(|e| MemoryError::Actor(e.to_string()))?;
        Ok(res?)
    }

    async fn request_synthesis(
        &self,
        target: SynthesisTarget,
        prior_summary: Option<String>,
        sources: Vec<SourceDoc>,
    ) -> Result<String, MemoryError> {
        let fut = self
            .llm
            .send(Synthesize {
                target,
                prior_summary,
                sources,
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        let res = fut.await.map_err(|e| MemoryError::Actor(e.to_string()))?;
        Ok(res?)
    }

    async fn index_insert(
        &self,
        path: &str,
        content: &str,
        chunks: Vec<Chunk>,
        model: String,
    ) -> Result<(), MemoryError> {
        if let Some(first) = chunks.first() {
            let dim = first.embedding.0.len();
            let fut = self
                .index
                .send(EnsureVecReady { dim })
                .await
                .map_err(|e| MemoryError::Actor(e.to_string()))?;
            fut.await.map_err(|e| MemoryError::Actor(e.to_string()))??;
        }
        let fut = self
            .index
            .send(IndexInsert {
                path: path.to_string(),
                source: "synthesized".to_string(),
                size: content.len() as u64,
                model,
                chunks,
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        fut.await.map_err(|e| MemoryError::Actor(e.to_string()))??;
        Ok(())
    }
}
```

- [ ] **Step 6: Update the `CooldownTick` handler**

In `src/memory/synthesizer.rs`, add the mandatory trace line to the `CooldownTick` handler:

```rust
impl Handler<CooldownTick> for Synthesizer {
    type Result = ();

    async fn handle(&mut self, msg: CooldownTick, _ctx: &mut Self::Context) {
        trace!("Handle command {:?}", msg);
        let should_process = matches!(self.last_event, Some(t) if t.elapsed() >= self.cooldown);
        if !should_process {
            return;
        }
        self.process().await;
    }
}
```

And in the `FileChanged` handler, change its first line to the standard form:

```rust
    async fn handle(&mut self, msg: FileChanged, ctx: &mut Self::Context) {
        trace!("Handle command {:?}", msg);
        self.pending.insert(msg.rel_path);
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p clawchorus --lib synthesizer && cargo test -p clawchorus --lib write_emits_synthesis_after_cooldown`
Expected: PASS — `synthesizer_writes_synthesis_after_cooldown` and `write_emits_synthesis_after_cooldown` both find a per-user synthesized file under `alice/_synthesized/{date}[-{n}].md` via `latest_synthesis_path`.

These tests run from the crate root, so the `Default` config's `prompts_dir = ./prompts` resolves to the templates created in Task 3.

- [ ] **Step 8: Run the full library suite and format**

Run: `cargo test -p clawchorus --lib && cargo fmt`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/memory/synthesizer.rs src/memory/manager.rs
git commit -m "feat(memory): two-pass synthesizer over Synthesize message"
```

---

## Task 9: Remove the dead `Session` path

Nothing references `Session`, `StartSession`, `SendMessage`, `StopSession`, or `derive_synthesis_path` anymore. Remove them.

**Files:**
- Delete: `src/llm/session.rs`
- Modify: `src/llm.rs` (remove `StartSession`, the `Session` handler, the `session` module)
- Modify: `src/memory/path.rs` (remove `derive_synthesis_path` + its tests)

- [ ] **Step 1: Delete the session module file**

```bash
git rm src/llm/session.rs
```

- [ ] **Step 2: Remove `Session` from `src/llm.rs`**

In `src/llm.rs`:
- Delete the `use crate::llm::session::Session;` line.
- Delete the `pub mod session;` line.
- Delete the `StartSession` struct definition (the `#[derive(Debug, Message)]` block with `#[result_type(Result<Address<Session>, LlmError>)]`).
- Delete the entire `impl Handler<StartSession> for LlmService { ... }` block.
- Delete the `start_session_returns_working_address` test from the `tests` module.
- If `Address` is now unused in `src/llm.rs`, remove it from the `acktor` import line. (`Address<Embedder>` and `Address<SynthesisTask>` are still used in the `LlmService` struct, so `Address` stays — verify with the compiler.)

- [ ] **Step 3: Remove `derive_synthesis_path` from `src/memory/path.rs`**

In `src/memory/path.rs`, delete the `derive_synthesis_path` function and the two tests that exercise it (the old `per_user_synthesis_path` test at line 69 and `cross_user_synthesis_path`). Keep the new `per_user_synthesis_path_uses_date_name` and `global_synthesis_path_uses_date_name` tests added in Task 7, and the `current_synthesis_path` / `latest_synthesis_path` helpers they cover.

Note: the old `per_user_synthesis_path` / `cross_user_synthesis_path` tests targeted `derive_synthesis_path`-style flat paths and are no longer meaningful under the date-named scheme — delete them rather than try to port them.

- [ ] **Step 4: Run the full suite to verify nothing broke**

Run: `cargo build -p clawchorus 2>&1`
Expected: clean build, no `unused import` or `dead_code` warnings for the removed items.

Run: `cargo test -p clawchorus --lib && cargo fmt`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/llm.rs src/memory/path.rs
git commit -m "refactor(llm): remove dead Session actor and derive_synthesis_path"
```

---

## Task 10: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Run the entire test suite**

Run: `cargo test -p clawchorus`
Expected: PASS — library tests plus the `wiremock`-based DeepSeek integration tests.

- [ ] **Step 2: Check for warnings**

Run: `cargo build -p clawchorus --all-targets 2>&1`
Expected: no warnings.

- [ ] **Step 3: Confirm formatting**

Run: `cargo fmt --check`
Expected: no output (already formatted).

- [ ] **Step 4: Spot-check the doc comment in `src/llm.rs`**

Confirm the module doc comment no longer mentions "conversation sessions" and reads as the new head written in Task 6.

- [ ] **Step 5: Commit if anything changed**

If steps 1-4 required fixes:

```bash
git add -A
git commit -m "chore: post-refactor verification fixes"
```

Otherwise, no commit.

---

## Self-Review Notes

- **Spec coverage:** `Embedder` (Task 4), `SynthesisTask` + `Synthesize` + `SynthesisTarget` + `SourceDoc` (Task 5), `LlmService` routing + `TaskDied` supervision (Task 6), prompt templates with provider override + hot-reload (Task 3), config fields incl. rename (Task 2), `Provider::name()` (Task 1), two-pass Synthesizer feeding only changed files (Task 8), date-named synthesized paths with rollover via `current_synthesis_path` / `latest_synthesis_path` and no `memory_type` segment (Task 7), removal of the generic `Session` API (Task 9). All `llm-service-design.md` and the Synthesizer section of `memory-manager-design.md` are covered.
- **Reset-and-reseed:** unified in `SynthesisTask::handle` — an empty `history` (cold start, post-restart, post-reset) reseeds from `prior_summary`; the Synthesizer always supplies the current on-disk summary.
- **Type consistency:** `Synthesize { target, prior_summary, sources }`, `SourceDoc { name, content }`, `SynthesisTarget::{User, Global}`, `TemplateKind::{PerUser, Global}`, `load_template(prompts_dir, provider_name, kind)`, `SynthesisTask::new(provider, kind, prompts_dir, idle_timeout, max_retries, context_max_chars)`, `Embedder::new(provider, max_retries)`, `LlmService::new(config, provider) -> Result<Self, LlmError>` — used identically across Tasks 5, 6, and 8.
- **Compile-safety:** `Session`/`StartSession` and `derive_synthesis_path` are kept until Task 9; every task leaves the crate building and the suite green.
