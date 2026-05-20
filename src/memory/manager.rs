//! Memory Manager Actor — supervisor for the memory sub-system.
//!
//! Spawns and supervises long-lived Storage and Index child actors.
//! For each incoming FileOp or Search message, spawns a short-lived
//! child actor to handle the request pipeline.

use std::path::PathBuf;

use acktor::message::FutureMessageResult;
use acktor::{Actor, Address, Context, ErrorReport, Handler, Message, Signal};
use tokio::task::JoinHandle;
use tracing::{info, trace, warn};

use crate::llm::LlmService;
use crate::memory::{
    config::MemoryConfig,
    error::MemoryError,
    file_op::FileOp,
    index::Index,
    messages::{FileOpDelete, FileOpRead, FileOpWrite, Search},
    search_op::SearchOp,
    storage::Storage,
    synthesizer::Synthesizer,
};

/// Shorthand for results in this module.
type Result<T> = std::result::Result<T, MemoryError>;

/// Supervisor actor for the memory sub-system.
///
/// Owns the long-lived Storage, Index, and Synthesizer child actors, and
/// dispatches each incoming [`FileOpWrite`]/[`FileOpRead`]/[`FileOpDelete`]/
/// [`Search`] message to a short-lived child actor that runs the request
/// pipeline off-mailbox.
pub struct MemoryManager {
    config: MemoryConfig,
    storage: Address<Storage>,
    index: Address<Index>,
    llm: Address<LlmService>,
    synthesizer: Address<Synthesizer>,
    storage_handle: Option<JoinHandle<()>>,
    index_handle: Option<JoinHandle<()>>,
    synthesizer_handle: Option<JoinHandle<()>>,
}

impl MemoryManager {
    /// Creates a Memory Manager, spawning its Storage, Index, and Synthesizer
    /// child actors from `config`.
    ///
    /// The Index is opened in memory when `config.db_path` is `":memory:"`,
    /// otherwise it is opened at that path. Returns an error if any child actor
    /// or the index fails to start.
    pub fn new(config: MemoryConfig, llm: Address<LlmService>) -> Result<Self> {
        let memory_dir = PathBuf::from(&config.memory_dir);
        let storage = Storage::new(memory_dir);
        let (storage_addr, storage_handle) = storage.start("storage")?;

        let index = if config.db_path == ":memory:" {
            Index::open_in_memory()?
        } else {
            Index::open(&PathBuf::from(&config.db_path))?
        };
        let (index_addr, index_handle) = index.start("index")?;

        let synthesizer = Synthesizer::new(
            storage_addr.clone(),
            index_addr.clone(),
            llm.clone(),
            config.synthesizer_cooldown_secs,
            config.chunk_size,
            config.chunk_overlap,
        );
        let (synthesizer_addr, synthesizer_handle) = synthesizer
            .start("synthesizer")
            .map_err(|_| MemoryError::Actor("failed to spawn Synthesizer".to_string()))?;

        Ok(Self {
            config,
            storage: storage_addr,
            index: index_addr,
            llm,
            synthesizer: synthesizer_addr,
            storage_handle: Some(storage_handle),
            index_handle: Some(index_handle),
            synthesizer_handle: Some(synthesizer_handle),
        })
    }

    /// Build a [`FutureMessageResult`] that spawns a fresh [`FileOp`] actor and
    /// drives the request off-mailbox, so the `MemoryManager` is free to
    /// process the next message while the pipeline runs.
    ///
    /// The spawn and the `send` happen inside this `async fn` so the message
    /// is queued in mailbox order before the handler returns; only the wait
    /// for the reply is deferred into the returned `FutureMessageResult`.
    async fn dispatch_file_op<M, T>(&self, msg: M) -> FutureMessageResult<M>
    where
        M: Message<Result = Result<T>> + Send + 'static,
        T: Send + 'static,
        FileOp: Handler<M>,
    {
        let prepared = match FileOp::new(
            self.storage.clone(),
            self.index.clone(),
            self.llm.clone(),
            self.synthesizer.clone(),
            self.config.chunk_size,
            self.config.chunk_overlap,
        )
        .start("file-op")
        {
            Ok((addr, _handle)) => match addr.send(msg).await {
                Ok(rx) => Ok::<_, MemoryError>((addr, rx)),
                Err(e) => Err(MemoryError::Actor(e.to_string())),
            },
            Err(_) => Err(MemoryError::Actor("failed to spawn FileOp".to_string())),
        };
        FutureMessageResult::new(async move {
            // Keep `_addr` alive across the response wait so the per-request
            // actor isn't terminated before it can reply.
            let (_addr, rx) = prepared?;
            rx.await.map_err(|e| MemoryError::Actor(e.to_string()))?
        })
    }
}

impl Actor for MemoryManager {
    type Context = Context<Self>;
    type Error = MemoryError;

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<()> {
        info!("MemoryManager is ready");

        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<()> {
        if let Some(join_handle) = self.synthesizer_handle.take() {
            if let Err(e) = self.synthesizer.do_send(Signal::Terminate).await {
                warn!("Could not stop synthesizer actor: {}", e.report());
                join_handle.abort();
            }

            if let Err(e) = join_handle.await {
                warn!("Synthesizer actor join error: {}", e);
            }
        }

        if let Some(join_handle) = self.storage_handle.take() {
            if let Err(e) = self.storage.do_send(Signal::Terminate).await {
                warn!("Could not stop storage actor: {}", e.report());
                join_handle.abort();
            }

            if let Err(e) = join_handle.await {
                warn!("Storage actor join error: {}", e);
            }
        }

        if let Some(join_handle) = self.index_handle.take() {
            if let Err(e) = self.index.do_send(Signal::Terminate).await {
                warn!("Could not stop index actor: {}", e.report());
                join_handle.abort();
            }

            if let Err(e) = join_handle.await {
                warn!("Index actor join error: {}", e);
            }
        }

        info!("MemoryManager is stopped");

        Ok(())
    }
}

impl Handler<FileOpWrite> for MemoryManager {
    type Result = FutureMessageResult<FileOpWrite>;

    async fn handle(
        &mut self,
        msg: FileOpWrite,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<FileOpWrite> {
        trace!("Handle command {:?}", msg);
        self.dispatch_file_op(msg).await
    }
}

impl Handler<FileOpRead> for MemoryManager {
    type Result = FutureMessageResult<FileOpRead>;

    async fn handle(
        &mut self,
        msg: FileOpRead,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<FileOpRead> {
        trace!("Handle command {:?}", msg);
        self.dispatch_file_op(msg).await
    }
}

impl Handler<FileOpDelete> for MemoryManager {
    type Result = FutureMessageResult<FileOpDelete>;

    async fn handle(
        &mut self,
        msg: FileOpDelete,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<FileOpDelete> {
        trace!("Handle command {:?}", msg);
        self.dispatch_file_op(msg).await
    }
}

impl Handler<Search> for MemoryManager {
    type Result = FutureMessageResult<Search>;

    async fn handle(
        &mut self,
        msg: Search,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<Search> {
        trace!("Handle command {:?}", msg);
        // Spawn and send eagerly so the message is queued in mailbox order;
        // only the response wait is deferred into the FutureMessageResult.
        let prepared = match SearchOp::new(
            self.index.clone(),
            self.llm.clone(),
            self.config.chunk_size,
            self.config.chunk_overlap,
        )
        .start("search-op")
        {
            Ok((addr, _handle)) => match addr.send(msg).await {
                Ok(rx) => Ok::<_, MemoryError>((addr, rx)),
                Err(e) => Err(MemoryError::Actor(e.to_string())),
            },
            Err(_) => Err(MemoryError::Actor("failed to spawn SearchOp".to_string())),
        };
        FutureMessageResult::new(async move {
            let (_addr, rx) = prepared?;
            rx.await.map_err(|e| MemoryError::Actor(e.to_string()))?
        })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::memory::{
        MemoryType,
        messages::{FileOpDelete, FileOpRead, FileOpWrite, Search},
    };

    fn test_llm() -> Address<LlmService> {
        let cfg = crate::llm::LlmConfig {
            provider: "mock".into(),
            embedding_provider: "mock".into(),
            ..Default::default()
        };
        let (addr, _handle) = LlmService::new(cfg).start("llm-test").unwrap();
        addr
    }

    fn test_config(dir: &std::path::Path) -> MemoryConfig {
        MemoryConfig {
            memory_dir: dir.to_string_lossy().to_string(),
            db_path: ":memory:".to_string(),
            ..MemoryConfig::default()
        }
    }

    #[tokio::test]
    async fn full_write_read_delete_cycle() {
        let dir = tempfile::tempdir().unwrap();

        let mm = MemoryManager::new(test_config(dir.path()), test_llm()).unwrap();
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::new_v4();

        // Write.
        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "test.md".to_string(),
            content: "Hello from test".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        // Read.
        let content = addr
            .send(FileOpRead {
                username: "alice".to_string(),
                agent_id,
                memory_type: MemoryType::DailyNote,
                filename: "test.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content, Some("Hello from test".to_string()));

        // Delete.
        addr.send(FileOpDelete {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "test.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        // Read after delete.
        let content = addr
            .send(FileOpRead {
                username: "alice".to_string(),
                agent_id,
                memory_type: MemoryType::DailyNote,
                filename: "test.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content, None);
    }

    #[tokio::test]
    async fn search_after_write() {
        let dir = tempfile::tempdir().unwrap();

        let mm = MemoryManager::new(test_config(dir.path()), test_llm()).unwrap();
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "notes.md".to_string(),
            content: "Rust programming language is great".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(Search {
                username: "alice".to_string(),
                agent_id,
                query: "programming".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn write_emits_synthesis_after_cooldown() {
        let dir = tempfile::tempdir().unwrap();

        let mut cfg = test_config(dir.path());
        cfg.synthesizer_cooldown_secs = 0;

        let mm = MemoryManager::new(cfg, test_llm()).unwrap();
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::new_v4();
        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            memory_type: MemoryType::DailyNote,
            filename: "first.md".to_string(),
            content: "Some content for synthesis".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let summary = dir
            .path()
            .join("alice")
            .join("_synthesized")
            .join("summary.md");
        assert!(
            summary.is_file(),
            "expected per-user summary at {:?}",
            summary
        );
    }
}
