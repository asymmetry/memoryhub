//! Search Actor — short-lived per-request actor for the search pipeline.
//!
//! Spawned by the Memory Manager for each incoming Search message.
//! Chunks the query, embeds via the LLM Service, searches the Index,
//! then terminates.

use acktor::{Actor, Address, Context, Handler};
use tracing::trace;

use crate::llm::{Embed, LlmService};
use crate::memory::{
    chunking::chunk_text,
    error::MemoryError,
    index::Index,
    messages::{IndexSearch, Search, SearchResult},
};

/// A short-lived actor that handles a single Search request.
pub struct SearchOp {
    index: Address<Index>,
    llm: Address<LlmService>,
    chunk_size: usize,
    chunk_overlap: usize,
}

impl SearchOp {
    pub fn new(
        index: Address<Index>,
        llm: Address<LlmService>,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            index,
            llm,
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
        trace!("Handle command {:?}", msg);

        if msg.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let text_chunks = chunk_text(&msg.query, self.chunk_size, self.chunk_overlap);

        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embeddings = if texts.is_empty() {
            return Ok(Vec::new());
        } else {
            self.llm
                .send(Embed { texts })
                .await
                .map_err(|e| MemoryError::Actor(e.to_string()))?
                .await
                .map_err(|e| MemoryError::Actor(e.to_string()))??
                .embeddings
        };

        let results = self
            .index
            .send(IndexSearch {
                embeddings,
                username: msg.username,
                agent_id: msg.agent_id,
                limit: 20,
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))??;

        Ok(results)
    }
}
