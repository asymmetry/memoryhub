//! Conversation session actor — child of `LlmService`, one per logical
//! conversation. Owns the full chat transcript and self-terminates on idle
//! via acktor's CronActor.

use std::sync::Arc;
use std::time::Duration;

use acktor::{
    Actor, ActorContext, Handler, Message, Signal,
    cron::{CronActor, CronContext},
    message::FutureMessageResult,
};
use tokio::time::Instant;
use tracing::{trace, warn};

use super::error::LlmError;
use super::provider::{ChatMessage, Provider, Role, retry};

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
            return Ok(Duration::from_secs(3600));
        }
        let remaining = self.idle_timeout.saturating_sub(elapsed);
        // Wake up at least once when the idle window elapses, but cap the
        // check frequency so we don't busy-spin for very short timeouts.
        Ok(remaining.max(Duration::from_millis(50)))
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

        let provider = self.provider.clone();
        let max_retries = self.max_retries;
        let history_snapshot = self.history.clone();

        // We must mutate self.history AFTER the call returns, so we run the
        // chat call synchronously inside handle and return a ready future.
        // The Session mailbox is per-conversation so serializing is fine.
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
            .send(SendMessage {
                content: "hi".into(),
            })
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
        m.push_chat(Ok(ChatResponse {
            model: "mock".into(),
            content: "reply-2".into(),
        }));
        m.push_chat(Ok(ChatResponse {
            model: "mock".into(),
            content: "reply-1".into(),
        }));

        let session = Session::new(m.clone(), "mock-chat".into(), Duration::from_secs(60), 3);
        let (addr, _h) = session.start("sess-test").unwrap();

        let r1 = addr
            .send(SendMessage {
                content: "turn-1".into(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r1, "reply-1");

        let r2 = addr
            .send(SendMessage {
                content: "turn-2".into(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r2, "reply-2");

        let last = m.last_chat_call().unwrap();
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

        addr.send(StopSession)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn idle_timeout_terminates_session() {
        let m = mock();
        let session = Session::new(m, "mock-chat".into(), Duration::from_millis(100), 3);
        let (_addr, handle) = session.start("sess-idle").unwrap();

        let res = tokio::time::timeout(Duration::from_millis(800), handle).await;
        assert!(res.is_ok(), "session should have terminated on idle");
    }
}
