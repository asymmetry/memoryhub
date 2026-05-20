//! Synthesis — `SynthesisTask` actor plus its public message and value types.
//!
//! One long-lived `SynthesisTask` exists per `SynthesisTarget`. It owns the conversation context
//! for that target so successive cool-down cycles refine a synthesis instead of rebuilding it.

use std::sync::Arc;
use std::time::Duration;

use acktor::{
    Actor, ActorContext, Handler, Message,
    cron::{CronActor, CronContext},
    utils::debug_trace,
};
use tokio::time::Instant;
use tracing::debug;

use super::config::LlmConfig;
use super::error::LlmError;
use super::provider::{ChatMessage, Provider, Role, retry};
use super::template::{TemplateKind, load_template};

/// Which synthesis a `Synthesize` request targets.
///
/// Identifies both the long-lived task to route to and the prompt template kind.
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
            SynthesisTarget::User(u) => format!("syn-task-{u}"),
            SynthesisTarget::Global => "syn-task-global".to_string(),
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

/// Synthesize `sources` into the target's running summary.
///
/// `prior_summary` is the current on-disk summary; it seeds a task whose context is empty
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

/// Long-lived per-target synthesis actor. Owns the conversation history for its target and
/// self-terminates on idle via `CronActor`.
pub struct SynthesisTask {
    config: LlmConfig,
    provider: Arc<dyn Provider>,
    kind: TemplateKind,
    history: Vec<ChatMessage>,
    last_activity: Instant,
}

impl SynthesisTask {
    pub fn new(config: LlmConfig, provider: Arc<dyn Provider>, kind: TemplateKind) -> Self {
        Self {
            config,
            provider,
            kind,
            history: Vec::new(),
            last_activity: Instant::now(),
        }
    }

    const fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.config.synthesis_idle_timeout_secs)
    }
}

impl Actor for SynthesisTask {
    type Context = CronContext<Self>;
    type Error = LlmError;
}

impl CronActor for SynthesisTask {
    async fn task(&mut self, ctx: &mut Self::Context) -> Result<Duration, LlmError> {
        let idle_timeout = self.idle_timeout();
        let elapsed = self.last_activity.elapsed();
        if elapsed >= idle_timeout {
            debug!("SynthesisTask idle for {:?}, terminating", elapsed);
            ctx.terminate();

            return Ok(Duration::from_secs(3600));
        }
        let remaining = idle_timeout.saturating_sub(elapsed);

        Ok(remaining.max(Duration::from_millis(50)))
    }
}

impl Handler<Synthesize> for SynthesisTask {
    type Result = Result<String, LlmError>;

    async fn handle(&mut self, msg: Synthesize, _ctx: &mut Self::Context) -> Self::Result {
        debug_trace!("Handle command {:?}", msg);

        self.last_activity = Instant::now();

        // Hot-reload the prompt template every call.
        let system =
            load_template(&self.config.prompts_dir, self.provider.name(), self.kind).await?;

        // Snapshot history length so a failed call can be rolled back cleanly.
        // Captured before the reseed push below, so a rollback removes both the
        // reseed turn and the sources turn.
        let restore_len = self.history.len();

        // Reseed an empty history from the prior summary.
        if self.history.is_empty()
            && let Some(summary) = msg.prior_summary
        {
            self.history.push(ChatMessage {
                role: Role::User,
                content: format!("Current summary so far:\n\n{summary}"),
            });
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
        let reply = match retry(self.config.max_retries, || provider.chat(&messages)).await {
            Ok(resp) => resp.content,
            Err(e) => {
                self.history.truncate(restore_len);
                return Err(e);
            }
        };

        self.history.push(ChatMessage {
            role: Role::Assistant,
            content: reply.clone(),
        });
        self.last_activity = Instant::now();

        // Reset context once it grows past the budget; the next call reseeds.
        let total: usize = self.history.iter().map(|m| m.content.len()).sum();
        if total > self.config.synthesis_context_max_chars {
            debug!("synthesis context {} chars over budget, clearing", total);
            self.history.clear();
        }

        Ok(reply)
    }
}

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
        let config = LlmConfig {
            prompts_dir: dir.path().to_path_buf(),
            synthesis_idle_timeout_secs: 60,
            max_retries: 3,
            synthesis_context_max_chars: max_chars,
            ..LlmConfig::default()
        };
        SynthesisTask::new(config, mock, TemplateKind::PerUser)
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
        let (addr, _h) = task(mock.clone(), &dir, 1_000_000)
            .start("synth-test")
            .unwrap();

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
        let (addr, _h) = task(mock.clone(), &dir, 1_000_000)
            .start("synth-test")
            .unwrap();

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
    async fn missing_on_disk_template_falls_back_to_embedded_default() {
        let mock = Arc::new(MockProvider::new());
        let empty = tempfile::tempdir().unwrap();
        let (addr, _h) = task(mock.clone(), &empty, 1_000_000)
            .start("synth-test")
            .unwrap();

        // With no on-disk template, the embedded default must be used and the
        // synthesis must succeed instead of returning LlmError::Config.
        addr.send(Synthesize {
            target: SynthesisTarget::User("alice".into()),
            prior_summary: None,
            sources: vec![doc("a.md", "x")],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let call = mock.last_chat_call().unwrap();
        assert_eq!(call[0].role, Role::System);
        assert_eq!(call[0].content, TemplateKind::PerUser.default_content());
    }

    #[tokio::test]
    async fn idle_timeout_terminates_task() {
        let mock = Arc::new(MockProvider::new());
        let dir = prompts_dir();
        // LlmConfig.synthesis_idle_timeout_secs is u64 secs; 1s is the minimum
        // resolution. The test waits up to 2s for self-termination.
        let config = LlmConfig {
            prompts_dir: dir.path().to_path_buf(),
            synthesis_idle_timeout_secs: 1,
            max_retries: 3,
            synthesis_context_max_chars: 1_000_000,
            ..LlmConfig::default()
        };
        let t = SynthesisTask::new(config, mock, TemplateKind::PerUser);
        let (_addr, handle) = t.start("synth-idle").unwrap();
        let res = tokio::time::timeout(Duration::from_millis(2000), handle).await;
        assert!(res.is_ok(), "task should self-terminate on idle");
    }
}
