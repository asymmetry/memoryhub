//! Supervisor for the memory sub-system.
//!
//! Spawns and supervises long-lived Storage and Indexer child actors. For each incoming FileOp
//! or Search message, spawns a short-lived child actor to handle the request pipeline.

use std::path::PathBuf;

use acktor::{
    Actor, Address, Context, Handler, JoinHandle, Message,
    message::FutureMessageResult,
    utils::{debug_trace, terminate_actor},
};
use tracing::info;

use super::config::MemoryConfig;
use super::error::MemoryError;
use super::file_op::FileOp;
use super::indexer::Indexer;
use super::message::{
    FileOpDelete, FileOpRead, FileOpWrite, GetSummary, Search, StorageRead, Summary,
};
use super::path;
use super::search_op::SearchOp;
use super::storage::Storage;
use super::synthesizer::Synthesizer;
use crate::llm::LlmService;

/// Shorthand for results in this module.
type Result<T> = std::result::Result<T, MemoryError>;

/// Supervisor actor for the memory sub-system.
pub struct MemoryManager {
    config: MemoryConfig,
    llm: Address<LlmService>,
    storage: Option<Address<Storage>>,
    index: Option<Address<Indexer>>,
    synthesizer: Option<Address<Synthesizer>>,
    storage_handle: Option<JoinHandle<()>>,
    index_handle: Option<JoinHandle<()>>,
    synthesizer_handle: Option<JoinHandle<()>>,
}

impl MemoryManager {
    /// Construct a `MemoryManager`. Child actors are spawned in
    /// [`Actor::post_start`], not here.
    pub fn new(config: MemoryConfig, llm: Address<LlmService>) -> Self {
        Self {
            config,
            llm,
            storage: None,
            index: None,
            synthesizer: None,
            storage_handle: None,
            index_handle: None,
            synthesizer_handle: None,
        }
    }

    fn storage(&self) -> &Address<Storage> {
        self.storage
            .as_ref()
            .expect("MemoryManager: storage not started")
    }

    fn index(&self) -> &Address<Indexer> {
        self.index
            .as_ref()
            .expect("MemoryManager: index not started")
    }

    fn synthesizer(&self) -> &Address<Synthesizer> {
        self.synthesizer
            .as_ref()
            .expect("MemoryManager: synthesizer not started")
    }

    async fn dispatch_file_op<M, T>(&self, msg: M) -> FutureMessageResult<M>
    where
        M: Message<Result = Result<T>> + Send + Sync + 'static,
        T: Send + 'static,
        FileOp: Handler<M>,
    {
        let result = match FileOp::new(
            self.llm.clone(),
            self.storage().clone(),
            self.index().clone(),
            self.synthesizer().clone(),
            self.config.chunk_size,
            self.config.chunk_overlap,
        )
        .start("file-op")
        {
            Ok((addr, _handle)) => match addr.send(msg).await {
                Ok(rx) => Ok::<_, MemoryError>((addr, rx)),
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e),
        };

        FutureMessageResult::new(async move {
            // Keep `_addr` alive across the response wait so the per-request
            // actor isn't terminated before it can reply.
            let (_addr, rx) = result?;
            rx.await?
        })
    }
}

impl Actor for MemoryManager {
    type Context = Context<Self>;
    type Error = MemoryError;

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<()> {
        let memory_dir = PathBuf::from(&self.config.memory_dir);
        let (storage_addr, storage_handle) = Storage::new(memory_dir.clone()).start("storage")?;

        let index = if self.config.db_path == ":memory:" {
            Indexer::open_in_memory()?
        } else {
            Indexer::open(&PathBuf::from(&self.config.db_path))?
        };
        let (index_addr, index_handle) = index.start("index")?;

        let synthesizer = Synthesizer::new(
            self.llm.clone(),
            storage_addr.clone(),
            index_addr.clone(),
            self.config.clone(),
        );
        let (synthesizer_addr, synthesizer_handle) = synthesizer.start("synthesizer")?;

        self.storage = Some(storage_addr);
        self.index = Some(index_addr);
        self.synthesizer = Some(synthesizer_addr);
        self.storage_handle = Some(storage_handle);
        self.index_handle = Some(index_handle);
        self.synthesizer_handle = Some(synthesizer_handle);

        info!("MemoryManager is ready");

        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<()> {
        if let (Some(addr), Some(handle)) =
            (self.synthesizer.take(), self.synthesizer_handle.take())
        {
            terminate_actor(addr, handle).await;
        }
        if let (Some(addr), Some(handle)) = (self.storage.take(), self.storage_handle.take()) {
            terminate_actor(addr, handle).await;
        }
        if let (Some(addr), Some(handle)) = (self.index.take(), self.index_handle.take()) {
            terminate_actor(addr, handle).await;
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
        debug_trace!("Handle command {:?}", msg);

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
        debug_trace!("Handle command {:?}", msg);

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
        debug_trace!("Handle command {:?}", msg);

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
        debug_trace!("Handle command {:?}", msg);

        let prepared = match SearchOp::new(
            self.llm.clone(),
            self.index().clone(),
            self.config.chunk_size,
            self.config.chunk_overlap,
        )
        .start("search-op")
        {
            Ok((addr, _handle)) => match addr.send(msg).await {
                Ok(rx) => Ok::<_, MemoryError>((addr, rx)),
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e),
        };

        FutureMessageResult::new(async move {
            let (_addr, rx) = prepared?;
            rx.await?
        })
    }
}

impl Handler<GetSummary> for MemoryManager {
    type Result = FutureMessageResult<GetSummary>;

    async fn handle(
        &mut self,
        msg: GetSummary,
        _ctx: &mut Self::Context,
    ) -> FutureMessageResult<GetSummary> {
        debug_trace!("Handle command {:?}", msg);

        let memory_dir = PathBuf::from(&self.config.memory_dir);
        let storage = self.storage().clone();

        FutureMessageResult::new(async move {
            let Some(target) = msg.scope.target(msg.username, msg.agent_id) else {
                return Ok(None);
            };
            let Some(rel) = path::get_latest_synthesis_file(&memory_dir, &target).await else {
                return Ok(None);
            };
            let content = storage
                .send(StorageRead { path: rel.clone() })
                .await?
                .await??;
            Ok(content.map(|content| Summary { content, path: rel }))
        })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::memory::message::{FileOpDelete, FileOpRead, FileOpWrite, Search};

    fn test_llm(dir: &std::path::Path) -> Address<LlmService> {
        let cfg = crate::llm::LlmConfig {
            provider: "mock".into(),
            embedding_provider: "mock".into(),
            prompts_dir: dir.join("prompts"),
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

        let mm = MemoryManager::new(test_config(dir.path()), test_llm(dir.path()));
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::new_v4();

        // Write.
        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            project: None,
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
                project: None,
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
            project: None,
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
                project: None,
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

        let mm = MemoryManager::new(test_config(dir.path()), test_llm(dir.path()));
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            project: None,
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
                scope: crate::memory::message::SearchScope::All,
                raw_only: false,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn get_summary_returns_latest_after_synthesis() {
        use crate::memory::message::{GetSummary, SummaryScope};

        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.synthesizer_cooldown_secs = 0;
        let mm = MemoryManager::new(cfg, test_llm(dir.path()));
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::new_v4();
        addr.send(FileOpWrite {
            username: "alice".into(),
            agent_id,
            project: None,
            filename: "first.md".into(),
            content: "Some content for synthesis".into(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let summary = addr
            .send(GetSummary {
                username: "alice".into(),
                agent_id: Some(agent_id),
                scope: SummaryScope::User,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let summary = summary.expect("expected a synthesized summary");
        assert!(
            summary.path.contains("alice/_synthesized/"),
            "path was {}",
            summary.path
        );
        assert!(!summary.content.is_empty());
    }

    #[tokio::test]
    async fn get_summary_is_none_when_absent() {
        use crate::memory::message::{GetSummary, SummaryScope};

        let dir = tempfile::tempdir().unwrap();
        let mm = MemoryManager::new(test_config(dir.path()), test_llm(dir.path()));
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let summary = addr
            .send(GetSummary {
                username: "bob".into(),
                agent_id: None,
                scope: SummaryScope::User,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(summary.is_none());
    }

    #[tokio::test]
    async fn write_emits_synthesis_after_cooldown() {
        let dir = tempfile::tempdir().unwrap();

        let mut cfg = test_config(dir.path());
        cfg.synthesizer_cooldown_secs = 0;

        let mm = MemoryManager::new(cfg, test_llm(dir.path()));
        let (addr, _handle) = mm.start("memory-manager").unwrap();

        let agent_id = Uuid::new_v4();
        addr.send(FileOpWrite {
            username: "alice".to_string(),
            agent_id,
            project: None,
            filename: "first.md".to_string(),
            content: "Some content for synthesis".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let folder = dir.path().join("alice").join("_synthesized");
        assert!(
            folder.is_dir(),
            "expected _synthesized folder at {:?}",
            folder
        );
        let any_md = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|s| s.ends_with(".md"))
            });
        assert!(any_md, "expected a dated synthesis file in {:?}", folder);
    }
}
