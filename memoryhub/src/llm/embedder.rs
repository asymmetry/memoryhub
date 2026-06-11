//! Embedder actor — long-lived child of `LlmService`. Owns `provider.embed`
//! so embedding is fully isolated from synthesis.

use std::sync::Arc;

use acktor::{Actor, Context, Handler, message::FutureMessageResult, utils::debug_trace};

use super::error::LlmError;
use super::provider::{self, EmbeddingProvider};
use super::{Embed, EmbedResult};

pub struct Embedder {
    provider: Arc<dyn EmbeddingProvider>,
    max_retries: u32,
    embedding_dim: Option<usize>,
}

impl Embedder {
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        max_retries: u32,
        embedding_dim: Option<usize>,
    ) -> Self {
        Self {
            provider,
            max_retries,
            embedding_dim,
        }
    }
}

/// Validates a provider's embedding response before it is trusted downstream: one vector per
/// input, each non-empty, all components finite, and of a consistent dimension.
fn validate_embeddings(
    texts: &[String],
    result: &EmbedResult,
    expected_dim: Option<usize>,
) -> Result<(), LlmError> {
    if result.embeddings.len() != texts.len() {
        return Err(LlmError::Provider(format!(
            "embedding count mismatch: requested {} texts but provider returned {}",
            texts.len(),
            result.embeddings.len()
        )));
    }
    // Dimension to enforce across the whole response: the pinned one if configured,
    // otherwise the first vector's, so every vector is at least internally consistent.
    let dim = expected_dim.or_else(|| result.embeddings.first().map(|e| e.0.len()));
    for (i, emb) in result.embeddings.iter().enumerate() {
        let v = &emb.0;
        if v.is_empty() {
            return Err(LlmError::Provider(format!(
                "provider returned an empty embedding for input {}",
                i
            )));
        }
        if let Some(expected) = dim
            && v.len() != expected
        {
            return Err(LlmError::Provider(format!(
                "embedding dimension mismatch for input {}: expected {} but got {}",
                i,
                expected,
                v.len()
            )));
        }
        if let Some(j) = v.iter().position(|x| !x.is_finite()) {
            return Err(LlmError::Provider(format!(
                "provider returned a non-finite embedding component at input {} index {}",
                i, j
            )));
        }
    }

    Ok(())
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
        let expected_dim = self.embedding_dim;

        FutureMessageResult::new(async move {
            let result = provider::retry(max_retries, || provider.embed(&msg.texts)).await?;
            validate_embeddings(&msg.texts, &result, expected_dim)?;
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Embed, EmbedResult, Embedding, provider::mock::MockProvider};
    use super::*;

    #[tokio::test]
    async fn embedder_returns_mock_vectors() {
        let mock = Arc::new(MockProvider::new());
        mock.push_embed(Ok(MockProvider::canned_embed(3, 2, "mock-emb")));

        let (addr, _h) = Embedder::new(mock.clone(), 3, None)
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

    /// Drives the actor with one scripted reply and one input text, returning the
    /// validated `Embed` result.
    async fn embed_one(
        reply: EmbedResult,
        expected_dim: Option<usize>,
    ) -> Result<EmbedResult, LlmError> {
        let mock = Arc::new(MockProvider::new());
        mock.push_embed(Ok(reply));
        let (addr, _h) = Embedder::new(mock, 1, expected_dim)
            .start("embedder-validate-test")
            .unwrap();
        addr.send(Embed {
            texts: vec!["a".into()],
        })
        .await
        .unwrap()
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn rejects_non_finite_component() {
        let reply = EmbedResult {
            model: "m".into(),
            embeddings: vec![Embedding(vec![0.1, f32::NAN, 0.2])],
        };
        let err = embed_one(reply, None).await.unwrap_err();
        assert!(matches!(err, LlmError::Provider(_)));
    }

    #[tokio::test]
    async fn rejects_dimension_mismatch_against_pinned_dim() {
        let reply = EmbedResult {
            model: "m".into(),
            embeddings: vec![Embedding(vec![0.1, 0.2])],
        };
        let err = embed_one(reply, Some(3)).await.unwrap_err();
        assert!(matches!(err, LlmError::Provider(_)));
    }

    #[tokio::test]
    async fn rejects_empty_vector() {
        let reply = EmbedResult {
            model: "m".into(),
            embeddings: vec![Embedding(vec![])],
        };
        let err = embed_one(reply, None).await.unwrap_err();
        assert!(matches!(err, LlmError::Provider(_)));
    }

    #[tokio::test]
    async fn rejects_count_mismatch() {
        // One input text but the provider returns two embeddings.
        let reply = EmbedResult {
            model: "m".into(),
            embeddings: vec![Embedding(vec![0.1, 0.2]), Embedding(vec![0.3, 0.4])],
        };
        let err = embed_one(reply, None).await.unwrap_err();
        assert!(matches!(err, LlmError::Provider(_)));
    }

    #[tokio::test]
    async fn accepts_valid_vector_matching_pinned_dim() {
        let reply = EmbedResult {
            model: "m".into(),
            embeddings: vec![Embedding(vec![0.1, 0.2, 0.3])],
        };
        let out = embed_one(reply, Some(3)).await.unwrap();
        assert_eq!(out.embeddings.len(), 1);
    }
}
