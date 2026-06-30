//! Search operation.
//!
//! Spawned by the [`MemoryManager`][super::MemoryManager] for each incoming [`Search`] message.
//! Chunks the query, embeds via the LLM Service, searches the Indexer, then terminates.

use acktor::{Actor, Address, Context, Handler, utils::debug_trace};

use super::chunking::chunk_text;
use super::error::MemoryError;
use super::indexer::Indexer;
use super::message::{IndexIsEmpty, IndexSearch, Search, SearchResult};
use crate::llm::{Embed, LlmService};

/// A short-lived actor that handles a single Search request.
pub struct SearchOp {
    llm: Address<LlmService>,
    index: Address<Indexer>,
    chunk_size: usize,
    chunk_overlap: usize,
}

impl SearchOp {
    pub fn new(
        llm: Address<LlmService>,
        index: Address<Indexer>,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            llm,
            index,
            chunk_size,
            chunk_overlap,
        }
    }
}

impl Actor for SearchOp {
    type Context = Context<Self>;
    type Error = MemoryError;
}

impl Handler<Search> for SearchOp {
    type Result = Result<Vec<SearchResult>, MemoryError>;

    async fn handle(
        &mut self,
        msg: Search,
        _ctx: &mut Self::Context,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        debug_trace!("Handle command {:?}", msg);

        if msg.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // skip the (remote) embedding round-trip if the index is empty
        if self.index.send(IndexIsEmpty).await?.await?? {
            return Ok(Vec::new());
        }

        let text_chunks = chunk_text(&msg.query, self.chunk_size, self.chunk_overlap);

        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embeddings = if texts.is_empty() {
            return Ok(Vec::new());
        } else {
            self.llm.send(Embed { texts }).await?.await??.embeddings
        };

        let results = self
            .index
            .send(IndexSearch {
                embeddings,
                username: msg.username,
                agent_id: msg.agent_id,
                scope: msg.scope,
                raw_only: msg.raw_only,
                limit: 20,
            })
            .await?
            .await??;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::*;
    use crate::llm::provider::mock::MockProvider;
    use crate::llm::{Embedding, LlmConfig};
    use crate::memory::message::{Chunk, IndexInsert, SearchScope};

    fn test_llm(mock: Arc<MockProvider>, prompts_dir: std::path::PathBuf) -> Address<LlmService> {
        let cfg = LlmConfig {
            prompts_dir,
            ..Default::default()
        };
        LlmService::with_providers(cfg, mock.clone(), mock)
            .start("llm-test")
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn empty_index_search_skips_embedding() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProvider::new());
        let llm = test_llm(mock.clone(), dir.path().join("prompts"));
        let (index, _ih) = Indexer::open_in_memory().unwrap().start("index").unwrap();
        let (search, _sh) = SearchOp::new(llm, index, 512, 64)
            .start("search-op")
            .unwrap();

        let results = search
            .send(Search {
                username: "alice".into(),
                agent_id: Uuid::nil(),
                scope: SearchScope::All,
                raw_only: false,
                query: "anything".into(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(results.is_empty());
        assert_eq!(
            mock.embed_call_count(),
            0,
            "a search against an empty index must not call the embedder"
        );
    }

    #[tokio::test]
    async fn nonempty_index_search_embeds_query() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProvider::new());
        let llm = test_llm(mock.clone(), dir.path().join("prompts"));
        let (index, _ih) = Indexer::open_in_memory().unwrap().start("index").unwrap();

        // Seed one chunk so the index is non-empty (dim 4 matches the mock's default embedding).
        index
            .send(IndexInsert {
                path: "alice/agent1/note.md".into(),
                source: "raw".into(),
                size: 10,
                model: "mock".into(),
                chunks: vec![Chunk {
                    text: "rust".into(),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(vec![1.0; 4]),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let (search, _sh) = SearchOp::new(llm, index, 512, 64)
            .start("search-op")
            .unwrap();
        let results = search
            .send(Search {
                username: "alice".into(),
                agent_id: Uuid::nil(),
                scope: SearchScope::All,
                raw_only: false,
                query: "rust".into(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            mock.embed_call_count(),
            1,
            "a search against a non-empty index must embed the query"
        );
        assert!(!results.is_empty());
    }
}
