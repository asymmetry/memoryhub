//! FileOp Actor — short-lived per-request actor for write/read/delete pipelines.
//!
//! Spawned by the Memory Manager for each incoming FileOp message.
//! Coordinates between Storage, Index, and the LLM Service,
//! then terminates.

use acktor::{Actor, Address, Context, Handler};
use tracing::{error, trace, warn};

use crate::llm::{Embed, LlmService};
use crate::memory::{
    chunking::chunk_text,
    error::MemoryError,
    index::Index,
    messages::{
        Chunk, EnsureVecReady, FileChanged, FileOpDelete, FileOpRead, FileOpWrite, IndexDelete,
        IndexInsert, StorageDelete, StorageRead, StorageWrite,
    },
    path::derive_rel_path,
    storage::Storage,
    synthesizer::Synthesizer,
};

/// A short-lived actor that handles a single FileOp request.
pub struct FileOp {
    storage: Address<Storage>,
    index: Address<Index>,
    llm: Address<LlmService>,
    synthesizer: Address<Synthesizer>,
    chunk_size: usize,
    chunk_overlap: usize,
}

impl FileOp {
    pub fn new(
        storage: Address<Storage>,
        index: Address<Index>,
        llm: Address<LlmService>,
        synthesizer: Address<Synthesizer>,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            storage,
            index,
            llm,
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
        trace!("Handle command {:?}", msg);
        let rel_path = derive_rel_path(&msg.username, msg.agent_id, msg.memory_type, &msg.filename);

        // 1. Write to Storage.
        self.storage
            .send(StorageWrite {
                path: rel_path.clone(),
                content: msg.content.clone(),
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))??;

        // 2. Chunk.
        let text_chunks = chunk_text(&msg.content, self.chunk_size, self.chunk_overlap);

        // 3. Embed via LLM Service.
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embed_result = if texts.is_empty() {
            None
        } else {
            Some(
                self.llm
                    .send(Embed { texts })
                    .await
                    .map_err(|e| MemoryError::Actor(e.to_string()))?
                    .await
                    .map_err(|e| MemoryError::Actor(e.to_string()))??,
            )
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
            self.index
                .send(EnsureVecReady { dim })
                .await
                .map_err(|e| MemoryError::Actor(e.to_string()))?
                .await
                .map_err(|e| MemoryError::Actor(e.to_string()))??;
        }

        // 6. Insert into Index.
        let result = self
            .index
            .send(IndexInsert {
                path: rel_path.clone(),
                source: "raw".to_string(),
                size: msg.content.len() as u64,
                model,
                chunks,
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;

        // 7. Rollback on failure.
        if let Err(e) = result {
            error!(rel_path = %rel_path, error = %e, "FileOp: index insert failed, rolling back");
            let _ = self
                .storage
                .send(StorageDelete {
                    path: rel_path.clone(),
                })
                .await;
            return Err(e.into());
        }

        // 8. Notify the Synthesizer (fire-and-forget; failure is non-fatal).
        if let Err(e) = self
            .synthesizer
            .do_send(FileChanged {
                rel_path: rel_path.clone(),
            })
            .await
        {
            warn!(rel_path = %rel_path, error = %e, "FileOp: synthesizer notify failed");
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
        trace!("Handle command {:?}", msg);
        let rel_path = derive_rel_path(&msg.username, msg.agent_id, msg.memory_type, &msg.filename);

        let content = self
            .storage
            .send(StorageRead { path: rel_path })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))??;

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
        trace!("Handle command {:?}", msg);
        let rel_path = derive_rel_path(&msg.username, msg.agent_id, msg.memory_type, &msg.filename);

        // Delete from Index first.
        self.index
            .send(IndexDelete {
                path: rel_path.clone(),
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))??;

        // Then delete from Storage.
        self.storage
            .send(StorageDelete { path: rel_path })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))??;

        Ok(())
    }
}
