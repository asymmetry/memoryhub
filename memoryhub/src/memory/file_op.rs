//! File operations.
//!
//! Spawned by the [`MemoryManager`][super::MemoryManager] for each incoming `FileOp` message.
//! Coordinates between Storage, the Indexer, and the Synthesizer to execute the requested
//! operation, then terminates.

use acktor::{Actor, Address, Context, ErrorReport, Handler, utils::debug_trace};
use tracing::{error, warn};

use super::chunking::chunk_text;
use super::error::MemoryError;
use super::indexer::Indexer;
use super::message::{
    Chunk, FileChanged, FileOpDelete, FileOpRead, FileOpWrite, IndexDelete, IndexInsert,
    StorageDelete, StorageRead, StorageWrite,
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

        let storage_path = get_raw_path(
            &msg.username,
            msg.agent_id,
            msg.project.as_deref(),
            &msg.filename,
        )?;

        // Do all the fallible work — chunk, embed, and index — before touching
        // storage, so a failure leaves nothing to undo. Storage is written last,
        // only once the index is in place.

        // 1. Chunk.
        let text_chunks = chunk_text(&msg.content, self.chunk_size, self.chunk_overlap);

        // 2. Embed via LLM Service. This is the most failure-prone step (a network
        // call); doing it here means a timeout or rate-limit never writes anything.
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

        // 3. Build chunks with embeddings.
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

        // 4. Insert into the Indexer (one transaction that creates the vec table if
        // needed, then replaces any prior entry for this path; on failure the previous
        // entry is left intact).
        self.index
            .send(IndexInsert {
                path: storage_path.clone(),
                source: "raw".to_string(),
                size: msg.content.len() as u64,
                model,
                chunks,
            })
            .await?
            .await??;

        // 5. Persist to Storage last, now that everything else has succeeded. If
        // this final write fails, drop the index entry just made so search can never
        // point at a file that was never written. This only ever removes index rows,
        // never stored content, so it cannot lose a caller's data.
        if let Err(e) = self
            .storage
            .send(StorageWrite {
                path: storage_path.clone(),
                content: msg.content.clone(),
            })
            .await?
            .await?
        {
            error!(
                "Storage write for {} failed ({}); removing its index entry",
                storage_path,
                e.report()
            );

            let undo: Result<(), MemoryError> = async {
                self.index
                    .send(IndexDelete {
                        path: storage_path.clone(),
                    })
                    .await?
                    .await??;
                Ok(())
            }
            .await;

            if let Err(re) = undo {
                error!(
                    "Removing the index entry for {} after a storage write failure also failed: {}",
                    storage_path,
                    re.report()
                );
            }

            return Err(e.into());
        }

        // 6. Notify the Synthesizer (fire-and-forget; failure is non-fatal).
        if let Err(e) = self
            .synthesizer
            .do_send(FileChanged {
                username: msg.username.clone(),
                agent_id: msg.agent_id,
                path: storage_path.clone(),
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

        let storage_path = get_raw_path(
            &msg.username,
            msg.agent_id,
            msg.project.as_deref(),
            &msg.filename,
        )?;

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

        let storage_path = get_raw_path(
            &msg.username,
            msg.agent_id,
            msg.project.as_deref(),
            &msg.filename,
        )?;

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
                username: msg.username.clone(),
                agent_id: msg.agent_id,
                path: storage_path.clone(),
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use acktor::JoinHandle;
    use uuid::Uuid;

    use super::*;
    use crate::llm::{Embedding, LlmConfig};
    use crate::memory::config::MemoryConfig;
    use crate::memory::indexer::Indexer;
    use crate::memory::path::get_raw_path;

    /// Seed the index with a chunk of the given embedding dimension so a later insert
    /// with a different dimension fails the dimension check inside `IndexInsert`.
    async fn pin_index_dim(index: &Address<Indexer>, dim: usize) {
        index
            .send(IndexInsert {
                path: "seed/agent/dim.md".to_string(),
                source: "raw".to_string(),
                size: 1,
                model: "seed".to_string(),
                chunks: vec![Chunk {
                    text: "seed".to_string(),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(vec![0.1; dim]),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }

    #[allow(clippy::type_complexity)]
    async fn boot() -> (
        Address<Storage>,
        Address<Indexer>,
        Address<LlmService>,
        Address<Synthesizer>,
        tempfile::TempDir,
        Vec<JoinHandle<()>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (storage, h1) = Storage::new(PathBuf::from(dir.path())).start("s").unwrap();
        let (index, h2) = Indexer::open_in_memory().unwrap().start("i").unwrap();
        let llm_cfg = LlmConfig {
            provider: "mock".into(),
            embedding_provider: "mock".into(),
            prompts_dir: dir.path().join("prompts"),
            ..Default::default()
        };
        let (llm, h3) = LlmService::new(llm_cfg).start("l").unwrap();
        let mem_cfg = MemoryConfig {
            memory_dir: dir.path().to_string_lossy().to_string(),
            db_path: ":memory:".to_string(),
            synthesizer_cooldown_secs: 0,
            ..MemoryConfig::default()
        };
        let (synth, h4) = Synthesizer::new(llm.clone(), storage.clone(), index.clone(), mem_cfg)
            .start("syn")
            .unwrap();
        (storage, index, llm, synth, dir, vec![h1, h2, h3, h4])
    }

    #[tokio::test]
    async fn overwrite_is_preserved_when_indexing_fails() {
        let (storage, index, llm, synth, _dir, _handles) = boot().await;

        let username = "alice";
        let agent_id = Uuid::new_v4();
        let project = Some("proj".to_string());
        let filename = "note.md";
        let path = get_raw_path(username, agent_id, project.as_deref(), filename).unwrap();

        // Pre-existing, good content at the target path.
        storage
            .send(StorageWrite {
                path: path.clone(),
                content: "ORIGINAL".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        // Pin the index to a different embedding dimension than the mock produces (4),
        // so indexing fails before the storage write is ever attempted.
        pin_index_dim(&index, 3).await;

        let (file_op, _h) = FileOp::new(llm, storage.clone(), index, synth, 400, 80)
            .start("file-op-test")
            .unwrap();

        let result = file_op
            .send(FileOpWrite {
                username: username.to_string(),
                agent_id,
                project,
                filename: filename.to_string(),
                content: "NEW CONTENT".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(
            result.is_err(),
            "indexing should fail on dimension mismatch"
        );

        // Storage is written only after indexing succeeds, so the original is untouched.
        let content = storage
            .send(StorageRead { path })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content, Some("ORIGINAL".to_string()));
    }

    #[tokio::test]
    async fn new_file_not_written_when_indexing_fails() {
        let (storage, index, llm, synth, _dir, _handles) = boot().await;

        let agent_id = Uuid::new_v4();
        let path = get_raw_path("alice", agent_id, Some("proj"), "fresh.md").unwrap();

        // Pin the index to a dimension the mock won't match, so indexing fails.
        pin_index_dim(&index, 3).await;

        let (file_op, _h) = FileOp::new(llm, storage.clone(), index, synth, 400, 80)
            .start("file-op-test")
            .unwrap();

        let result = file_op
            .send(FileOpWrite {
                username: "alice".to_string(),
                agent_id,
                project: Some("proj".to_string()),
                filename: "fresh.md".to_string(),
                content: "BRAND NEW".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(
            result.is_err(),
            "indexing should fail on dimension mismatch"
        );

        // A failed write must never have touched storage.
        let content = storage
            .send(StorageRead { path })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content, None);
    }
}
