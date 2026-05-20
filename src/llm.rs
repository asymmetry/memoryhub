//! LLM Service Actor — the single entry point for embedding and synthesis.
//!
//! Sibling of Memory Manager, supervised by the top-level Manager. It spawns
//! an `Embedder` child and one long-lived `SynthesisTask` child per target.

use std::sync::Arc;

use acktor::{
    Actor, ActorContext, Address, Context, ErrorReport, Handler, JoinHandle, Message,
    message::FutureMessageResult,
    utils::{debug_trace, terminate_actor},
};
use ahash::HashMap;
use tracing::warn;

mod error;
pub use error::LlmError;

mod config;
pub use config::LlmConfig;

pub mod provider;
pub use provider::{EmbeddingProvider, Provider, build_providers};

mod embedder;
pub use embedder::Embedder;

pub mod template;

pub mod synthesis;
pub use synthesis::{SourceDoc, SynthesisTarget, SynthesisTask, Synthesize};

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

#[derive(Debug, Message)]
#[result_type(())]
struct SynthesisTaskTerminated {
    target: SynthesisTarget,
}

pub struct LlmService {
    config: LlmConfig,
    provider: Option<Arc<dyn Provider>>,
    embedder: Option<Address<Embedder>>,
    embedder_handle: Option<JoinHandle<()>>,
    tasks: HashMap<SynthesisTarget, Address<SynthesisTask>>,
}

impl LlmService {
    /// Constructs a new `LlmService` with the given config.
    pub fn new(config: LlmConfig) -> Self {
        Self {
            config,
            provider: None,
            embedder: None,
            embedder_handle: None,
            tasks: HashMap::default(),
        }
    }

    fn provider(&self) -> &Arc<dyn Provider> {
        self.provider
            .as_ref()
            .expect("LlmService.provider must be initialized by post_start")
    }
}

impl Actor for LlmService {
    type Context = Context<Self>;
    type Error = LlmError;

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<(), LlmError> {
        let (chat, embedding) = build_providers(&self.config)?;
        self.provider = Some(chat);

        if let Err(e) = template::write_default_templates(&self.config.prompts_dir).await {
            warn!("Could not write default prompt templates: {}", e.report());
        }

        let (embedder, handle) =
            Embedder::new(embedding, self.config.max_retries).start("embedder")?;
        self.embedder = Some(embedder);
        self.embedder_handle = Some(handle);

        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<(), LlmError> {
        if let (Some(embedder), Some(handle)) = (self.embedder.take(), self.embedder_handle.take())
        {
            terminate_actor(embedder, handle).await;
        }

        Ok(())
    }
}

impl Handler<Embed> for LlmService {
    type Result = FutureMessageResult<Embed>;

    async fn handle(&mut self, msg: Embed, _ctx: &mut Self::Context) -> Self::Result {
        debug_trace!("Handle command {:?}", msg);

        let embedder = self
            .embedder
            .clone()
            .expect("LlmService.embedder must be initialized by post_start");
        FutureMessageResult::new(async move { embedder.send(msg).await?.await? })
    }
}

impl Handler<Synthesize> for LlmService {
    type Result = FutureMessageResult<Synthesize>;

    async fn handle(&mut self, msg: Synthesize, ctx: &mut Self::Context) -> Self::Result {
        debug_trace!("Handle command {:?}", msg);

        let task = match self.tasks.get(&msg.target) {
            Some(addr) => addr.clone(),
            None => {
                let target = msg.target.clone();
                let label = target.label();
                let new_task = SynthesisTask::new(
                    self.config.clone(),
                    self.provider().clone(),
                    target.template_kind(),
                );
                match new_task.start(&label) {
                    Ok((addr, handle)) => {
                        // When the task self-terminates on idle, tell ourselves
                        // so the stale map entry is dropped.
                        let self_addr = ctx.address().clone();
                        let dead = target.clone();
                        tokio::spawn(async move {
                            let _ = handle.await;
                            let _ = self_addr
                                .do_send(SynthesisTaskTerminated { target: dead })
                                .await;
                        });
                        self.tasks.insert(target, addr.clone());
                        addr
                    }
                    Err(e) => {
                        return FutureMessageResult::new(async move { Err(e) });
                    }
                }
            }
        };

        FutureMessageResult::new(async move { task.send(msg).await?.await? })
    }
}

impl Handler<SynthesisTaskTerminated> for LlmService {
    type Result = ();

    async fn handle(&mut self, msg: SynthesisTaskTerminated, _ctx: &mut Self::Context) {
        debug_trace!("Handle command {:?}", msg);

        self.tasks.remove(&msg.target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LlmConfig {
        LlmConfig {
            provider: "mock".to_string(),
            embedding_provider: "mock".to_string(),
            synthesis_idle_timeout_secs: 60,
            ..LlmConfig::default()
        }
    }

    #[tokio::test]
    async fn synthesize_spawns_task_and_returns_reply() {
        let (addr, _h) = LlmService::new(cfg()).start("llm-test").unwrap();

        let reply = addr
            .send(Synthesize {
                target: crate::llm::SynthesisTarget::User("alice".into()),
                prior_summary: None,
                sources: vec![crate::llm::SourceDoc {
                    name: "alice/a/daily_note/x.md".into(),
                    content: "hello".into(),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!reply.is_empty());
    }
}
