//! Search Actor.
//!
//! Spawned by the Memory Manager for each incoming Search message. Chunks the query, embeds via
//! the LLM Service, searches the Indexer, then terminates.

use acktor::{Actor, Address, Context, Handler, utils::debug_trace};

use super::chunking::chunk_text;
use super::error::MemoryError;
use super::indexer::Indexer;
use super::message::{IndexSearch, Search, SearchResult};
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
                limit: 20,
            })
            .await?
            .await??;

        Ok(results)
    }
}
