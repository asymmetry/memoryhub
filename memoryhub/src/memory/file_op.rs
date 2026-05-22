//! File operation actors.
//!
//! Spawned by the Memory Manager for each incoming FileOp message. Coordinates between Storage,
//! Indexer, and the LLM Service, then terminates.

use acktor::{Actor, Address, Context, ErrorReport, Handler, utils::debug_trace};
use tracing::{error, warn};

use super::chunking::chunk_text;
use super::error::MemoryError;
use super::indexer::Indexer;
use super::message::{
    Chunk, EnsureVecReady, FileChanged, FileOpDelete, FileOpRead, FileOpWrite, IndexDelete,
    IndexInsert, StorageDelete, StorageRead, StorageWrite,
};
use super::path::get_raw_path;
use super::storage::Storage;
use super::synthesizer::Synthesizer;
use crate::llm::{Embed, LlmService};

/// A short-lived actor that handles a single FileOp request.
pub struct FileOp {
    llm: Address<LlmService>,
    storage: Address<Storage>,
    index: Address<Indexer>,
    synthesizer: Address<Synthesizer>,
    chunk_size: usize,
    chunk_overlap: usize,
}

impl FileOp {
    /// Constructs a new `FileOp` actor.
    pub fn new(
        llm: Address<LlmService>,
        storage: Address<Storage>,
        index: Address<Indexer>,
        synthesizer: Address<Synthesizer>,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            llm,
            storage,
            index,
            synthesizer,
            chunk_size,
            chunk_overlap,
        }
    }
}

impl Actor for FileOp {
    type Context = Context<Self>;
    type Error = MemoryError;
}

impl Handler<FileOpWrite> for FileOp {
    type Result = Result<(), MemoryError>;

    async fn handle(
        &mut self,
        msg: FileOpWrite,
        _ctx: &mut Self::Context,
    ) -> Result<(), MemoryError> {
        debug_trace!("Handle command {:?}", msg);

        let storage_path = get_raw_path(&msg.username, msg.agent_id, &msg.filename);

        // 1. Write to Storage.
        self.storage
            .send(StorageWrite {
                path: storage_path.clone(),
                content: msg.content.clone(),
            })
            .await?
            .await??;

        // 2. Chunk.
        let text_chunks = chunk_text(&msg.content, self.chunk_size, self.chunk_overlap);

        // 3. Embed via LLM Service.
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embed_result = if texts.is_empty() {
            None
        } else {
            Some(self.llm.send(Embed { texts }).await?.await??)
        };

        let (model, embeddings) = match embed_result {
            Some(r) => (r.model, r.embeddings),
            None => (String::new(), Vec::new()),
        };

        // 4. Build chunks with embeddings.
        let chunks: Vec<Chunk> = text_chunks
            .into_iter()
            .zip(embeddings)
            .map(|(tc, emb)| Chunk {
                text: tc.text,
                start_line: tc.start_line,
                end_line: tc.end_line,
                embedding: emb,
            })
            .collect();

        // 5. Ensure the vec table exists for this embedding dimension.
        if let Some(first) = chunks.first() {
            let dim = first.embedding.0.len();
            self.index.send(EnsureVecReady { dim }).await?.await??;
        }

        // 6. Insert into Indexer.
        let result = self
            .index
            .send(IndexInsert {
                path: storage_path.clone(),
                source: "raw".to_string(),
                size: msg.content.len() as u64,
                model,
                chunks,
            })
            .await?
            .await?;

        // 7. Rollback on failure.
        if let Err(e) = result {
            error!(
                "Indexer insert for {} failed ({}); rolling back the storage write",
                storage_path,
                e.report()
            );

            let _ = self
                .storage
                .do_send(StorageDelete {
                    path: storage_path.clone(),
                })
                .await;

            return Err(e.into());
        }

        // 8. Notify the Synthesizer (fire-and-forget; failure is non-fatal).
        if let Err(e) = self
            .synthesizer
            .do_send(FileChanged {
                rel_path: storage_path.clone(),
            })
            .await
        {
            warn!(
                "Failed to notify the synthesizer about {}: {}",
                storage_path,
                e.report()
            );
        }

        Ok(())
    }
}

impl Handler<FileOpRead> for FileOp {
    type Result = Result<Option<String>, MemoryError>;

    async fn handle(
        &mut self,
        msg: FileOpRead,
        _ctx: &mut Self::Context,
    ) -> Result<Option<String>, MemoryError> {
        debug_trace!("Handle command {:?}", msg);

        let storage_path = get_raw_path(&msg.username, msg.agent_id, &msg.filename);

        let content = self
            .storage
            .send(StorageRead { path: storage_path })
            .await?
            .await??;

        Ok(content)
    }
}

impl Handler<FileOpDelete> for FileOp {
    type Result = Result<(), MemoryError>;

    async fn handle(
        &mut self,
        msg: FileOpDelete,
        _ctx: &mut Self::Context,
    ) -> Result<(), MemoryError> {
        debug_trace!("Handle command {:?}", msg);

        let storage_path = get_raw_path(&msg.username, msg.agent_id, &msg.filename);

        // Delete from Indexer first.
        self.index
            .send(IndexDelete {
                path: storage_path.clone(),
            })
            .await?
            .await??;

        // Then delete from Storage.
        self.storage
            .send(StorageDelete {
                path: storage_path.clone(),
            })
            .await?
            .await??;

        // Notify the Synthesizer so it can refresh the running summary
        // without the deleted source.
        if let Err(e) = self
            .synthesizer
            .do_send(FileChanged {
                rel_path: storage_path.clone(),
            })
            .await
        {
            warn!(
                "Failed to notify the synthesizer about {}: {}",
                storage_path,
                e.report()
            );
        }

        Ok(())
    }
}
