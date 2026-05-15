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

    async fn handle(&mut self, msg: Embed, _ctx: &mut Self::Context) -> FutureMessageResult<Embed> {
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
    use crate::llm::provider::{ChatResponse, Role};

    fn cfg() -> LlmConfig {
        LlmConfig {
            session_idle_timeout_secs: 60,
            ..LlmConfig::default()
        }
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
