//! Embedder actor — long-lived child of `LlmService`. Owns `provider.embed`
//! so embedding is fully isolated from synthesis.

use std::sync::Arc;

use acktor::{Actor, Context, Handler, message::FutureMessageResult, utils::debug_trace};

use super::Embed;
use super::error::LlmError;
use super::provider::{self, EmbeddingProvider};

pub struct Embedder {
    provider: Arc<dyn EmbeddingProvider>,
    max_retries: u32,
}

impl Embedder {
    pub fn new(provider: Arc<dyn EmbeddingProvider>, max_retries: u32) -> Self {
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
        debug_trace!("Handle command {:?}", msg);

        let provider = self.provider.clone();
        let max_retries = self.max_retries;

        FutureMessageResult::new(async move {
            provider::retry(max_retries, || provider.embed(&msg.texts)).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Embed, provider::mock::MockProvider};
    use super::*;

    #[tokio::test]
    async fn embedder_returns_mock_vectors() {
        let mock = Arc::new(MockProvider::new());
        mock.push_embed(Ok(MockProvider::canned_embed(3, 2, "mock-emb")));

        let (addr, _h) = Embedder::new(mock.clone(), 3)
            .start("embedder-test")
            .unwrap();

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
