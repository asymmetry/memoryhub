//! Synthesizer Actor — long-lived child of MemoryManager.
//!
//! Receives fire-and-forget `FileChanged` notifications, batches them with a
//! cool-down timer, then runs a two-pass synthesis. The per-user pass folds
//! each affected user's changed files into that user's running summary; the
//! global pass folds the changed per-user summaries into a cross-user
//! summary. Synthesis itself is delegated to LLM Service via `Synthesize`;
//! this actor only reads sources, writes results, and indexes them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use acktor::{Actor, ActorContext, Address, Context, Handler, Message};
use tokio::time::Instant;
use tracing::{info, trace, warn};

use crate::llm::{Embed, EmbedResult, LlmService, SourceDoc, SynthesisTarget, Synthesize};
use crate::memory::{
    chunking::chunk_text,
    error::MemoryError,
    index::Index,
    messages::{Chunk, EnsureVecReady, FileChanged, IndexInsert, StorageRead, StorageWrite},
    path::{global_synthesis_path, per_user_synthesis_path},
    storage::Storage,
};

#[derive(Debug, Clone, Message)]
#[result_type(())]
struct CooldownTick;

pub struct Synthesizer {
    storage: Address<Storage>,
    index: Address<Index>,
    llm: Address<LlmService>,
    cooldown: Duration,
    chunk_size: usize,
    chunk_overlap: usize,
    pending: BTreeSet<String>,
    last_event: Option<Instant>,
}

impl Synthesizer {
    pub fn new(
        storage: Address<Storage>,
        index: Address<Index>,
        llm: Address<LlmService>,
        cooldown_secs: u64,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            storage,
            index,
            llm,
            cooldown: Duration::from_secs(cooldown_secs),
            chunk_size,
            chunk_overlap,
            pending: BTreeSet::new(),
            last_event: None,
        }
    }

    /// Run the two-pass synthesis over the pending set.
    async fn process(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let paths: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
        info!("Synthesizer: processing {} pending paths", paths.len());

        // Group the changed paths by owning user.
        let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in paths {
            match path.split('/').next() {
                Some(u) if !u.is_empty() && u != "_synthesized" => {
                    by_user.entry(u.to_string()).or_default().push(path);
                }
                _ => {}
            }
        }

        // Per-user pass.
        let mut changed_users: Vec<String> = Vec::new();
        for (user, changed) in &by_user {
            match self.synthesize_user(user, changed).await {
                Ok(true) => changed_users.push(user.clone()),
                Ok(false) => {}
                Err(e) => warn!("Synthesizer: per-user synthesis for {} failed: {}", user, e),
            }
        }

        // Global pass.
        if !changed_users.is_empty()
            && let Err(e) = self.synthesize_global(&changed_users).await
        {
            warn!("Synthesizer: global synthesis failed: {}", e);
        }
    }

    /// Re-synthesize one user from their changed files. Returns `true` if a
    /// summary was written, `false` if there was nothing to synthesize.
    async fn synthesize_user(&self, user: &str, changed: &[String]) -> Result<bool, MemoryError> {
        let mut sources = Vec::new();
        for path in changed {
            match self.storage_read(path).await? {
                Some(content) => sources.push(SourceDoc {
                    name: path.clone(),
                    content,
                }),
                None => trace!("Synthesizer: source {} vanished, skipping", path),
            }
        }
        if sources.is_empty() {
            return Ok(false);
        }

        let summary_path = per_user_synthesis_path(user);
        let prior_summary = self.storage_read(&summary_path).await?;
        let synthesis = self
            .request_synthesis(
                SynthesisTarget::User(user.to_string()),
                prior_summary,
                sources,
            )
            .await?;
        self.write_synthesis(&summary_path, &synthesis).await?;
        info!(
            "Synthesizer: per-user synthesis written to {}",
            summary_path
        );
        Ok(true)
    }

    /// Synthesize the cross-user summary from the changed per-user summaries.
    async fn synthesize_global(&self, changed_users: &[String]) -> Result<(), MemoryError> {
        let mut sources = Vec::new();
        for user in changed_users {
            let path = per_user_synthesis_path(user);
            if let Some(content) = self.storage_read(&path).await? {
                sources.push(SourceDoc {
                    name: path,
                    content,
                });
            }
        }
        if sources.is_empty() {
            return Ok(());
        }

        let summary_path = global_synthesis_path();
        let prior_summary = self.storage_read(&summary_path).await?;
        let synthesis = self
            .request_synthesis(SynthesisTarget::Global, prior_summary, sources)
            .await?;
        self.write_synthesis(&summary_path, &synthesis).await?;
        info!("Synthesizer: global synthesis written to {}", summary_path);
        Ok(())
    }

    /// Chunk, embed, and write a synthesized document to Storage and Index.
    async fn write_synthesis(&self, path: &str, content: &str) -> Result<(), MemoryError> {
        self.storage_write(path, content).await?;

        let text_chunks = chunk_text(content, self.chunk_size, self.chunk_overlap);
        if text_chunks.is_empty() {
            return Ok(());
        }
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embed_result = self.embed(texts).await?;
        let chunks: Vec<Chunk> = text_chunks
            .into_iter()
            .zip(embed_result.embeddings)
            .map(|(tc, emb)| Chunk {
                text: tc.text,
                start_line: tc.start_line,
                end_line: tc.end_line,
                embedding: emb,
            })
            .collect();
        self.index_insert(path, content, chunks, embed_result.model)
            .await
    }

    async fn storage_read(&self, path: &str) -> Result<Option<String>, MemoryError> {
        let fut = self
            .storage
            .send(StorageRead {
                path: path.to_string(),
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        let res = fut.await.map_err(|e| MemoryError::Actor(e.to_string()))?;
        Ok(res?)
    }

    async fn storage_write(&self, path: &str, content: &str) -> Result<(), MemoryError> {
        let fut = self
            .storage
            .send(StorageWrite {
                path: path.to_string(),
                content: content.to_string(),
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        fut.await.map_err(|e| MemoryError::Actor(e.to_string()))??;
        Ok(())
    }

    async fn embed(&self, texts: Vec<String>) -> Result<EmbedResult, MemoryError> {
        let fut = self
            .llm
            .send(Embed { texts })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        let res = fut.await.map_err(|e| MemoryError::Actor(e.to_string()))?;
        Ok(res?)
    }

    async fn request_synthesis(
        &self,
        target: SynthesisTarget,
        prior_summary: Option<String>,
        sources: Vec<SourceDoc>,
    ) -> Result<String, MemoryError> {
        let fut = self
            .llm
            .send(Synthesize {
                target,
                prior_summary,
                sources,
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        let res = fut.await.map_err(|e| MemoryError::Actor(e.to_string()))?;
        Ok(res?)
    }

    async fn index_insert(
        &self,
        path: &str,
        content: &str,
        chunks: Vec<Chunk>,
        model: String,
    ) -> Result<(), MemoryError> {
        if let Some(first) = chunks.first() {
            let dim = first.embedding.0.len();
            let fut = self
                .index
                .send(EnsureVecReady { dim })
                .await
                .map_err(|e| MemoryError::Actor(e.to_string()))?;
            fut.await.map_err(|e| MemoryError::Actor(e.to_string()))??;
        }
        let fut = self
            .index
            .send(IndexInsert {
                path: path.to_string(),
                source: "synthesized".to_string(),
                size: content.len() as u64,
                model,
                chunks,
            })
            .await
            .map_err(|e| MemoryError::Actor(e.to_string()))?;
        fut.await.map_err(|e| MemoryError::Actor(e.to_string()))??;
        Ok(())
    }
}

impl Actor for Synthesizer {
    type Context = Context<Self>;
    type Error = MemoryError;
}

impl Handler<FileChanged> for Synthesizer {
    type Result = ();

    async fn handle(&mut self, msg: FileChanged, ctx: &mut Self::Context) {
        trace!("Handle command {:?}", msg);
        self.pending.insert(msg.rel_path);
        self.last_event = Some(Instant::now());

        let addr = ctx.address().clone();
        let cooldown = self.cooldown;
        tokio::spawn(async move {
            tokio::time::sleep(cooldown).await;
            let _ = addr.do_send(CooldownTick).await;
        });
    }
}

impl Handler<CooldownTick> for Synthesizer {
    type Result = ();

    async fn handle(&mut self, msg: CooldownTick, _ctx: &mut Self::Context) {
        trace!("Handle command {:?}", msg);
        let should_process = matches!(self.last_event, Some(t) if t.elapsed() >= self.cooldown);
        if !should_process {
            return;
        }
        self.process().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmService;
    use crate::memory::index::Index;
    use crate::memory::messages::StorageWrite;
    use crate::memory::storage::Storage;
    use std::path::PathBuf;
    use tokio::task::JoinHandle;

    async fn boot() -> (
        Address<Storage>,
        Address<Index>,
        Address<LlmService>,
        tempfile::TempDir,
        Vec<JoinHandle<()>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (storage, h1) = Storage::new(PathBuf::from(dir.path())).start("s").unwrap();
        let (index, h2) = Index::open_in_memory().unwrap().start("i").unwrap();
        let llm_cfg = crate::llm::LlmConfig {
            provider: "mock".into(),
            embedding_provider: "mock".into(),
            ..Default::default()
        };
        let (llm, h3) = LlmService::new(llm_cfg).start("l").unwrap();
        (storage, index, llm, dir, vec![h1, h2, h3])
    }

    #[tokio::test]
    async fn synthesizer_writes_synthesis_after_cooldown() {
        let (storage, index, llm, dir, _handles) = boot().await;

        storage
            .send(StorageWrite {
                path: "alice/agent1/daily_note/x.md".to_string(),
                content: "hello world".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let synth = Synthesizer::new(storage.clone(), index, llm, 0, 400, 80);
        let (addr, _handle) = synth.start("synth").unwrap();

        addr.send(FileChanged {
            rel_path: "alice/agent1/daily_note/x.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

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
