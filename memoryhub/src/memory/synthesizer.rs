//! Synthesize the memory files into summaries.
//!
//! Receives fire-and-forget `FileChanged` notifications, batches them with a cool-down timer,
//! then runs a three-pass synthesis.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use acktor::{
    Actor, ActorContext, Address, Context, ErrorReport, Handler, Message, utils::debug_trace,
};
use ahash::{HashMap, HashSet};
use chrono::Utc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::chunking::chunk_text;
use super::config::MemoryConfig;
use super::error::MemoryError;
use super::indexer::Indexer;
use super::message::{Chunk, EnsureVecReady, FileChanged, IndexInsert, StorageRead, StorageWrite};
use super::path::{current_synthesis_path, get_latest_synthesis_file};
use super::storage::Storage;
use crate::llm::{Embed, EmbedResult, LlmService, SourceDoc, SynthesisTarget, Synthesize};

#[derive(Debug, Clone, Message)]
#[result_type(())]
struct CooldownTick;

pub struct Synthesizer {
    llm: Address<LlmService>,
    storage: Address<Storage>,
    index: Address<Indexer>,
    config: MemoryConfig,
    changed_files: HashSet<FileChanged>,
    timer_armed: bool,
}

impl Synthesizer {
    pub fn new(
        llm: Address<LlmService>,
        storage: Address<Storage>,
        index: Address<Indexer>,
        config: MemoryConfig,
    ) -> Self {
        Self {
            llm,
            storage,
            index,
            config,
            changed_files: HashSet::default(),
            timer_armed: false,
        }
    }

    fn memory_dir(&self) -> PathBuf {
        PathBuf::from(&self.config.memory_dir)
    }

    fn cooldown(&self) -> Duration {
        Duration::from_secs(self.config.synthesizer_cooldown_secs)
    }

    /// Run the three-tier cascade over the pending set.
    async fn process(&mut self) {
        if self.changed_files.is_empty() {
            return;
        }
        let changed_files = std::mem::take(&mut self.changed_files);

        debug!(
            "Synthesizer: processing {} pending paths",
            changed_files.len()
        );

        let mut file_groups: HashMap<(String, Uuid), BTreeSet<String>> = HashMap::default();
        let mut agent_groups: HashMap<String, BTreeSet<Uuid>> = HashMap::default();
        let mut users: BTreeSet<String> = BTreeSet::default();

        for file in changed_files {
            file_groups
                .entry((file.username, file.agent_id))
                .or_default()
                .insert(file.path);
        }

        // Pass 1: per-agent.
        for ((username, agent_id), changed_files) in &file_groups {
            match self
                .synthesize_agent(username, *agent_id, changed_files)
                .await
            {
                Ok(true) => {
                    agent_groups
                        .entry(username.clone())
                        .or_default()
                        .insert(*agent_id);
                }
                Ok(false) => {}
                Err(e) => warn!(
                    "Synthesizer: per-agent synthesis for {}/{} failed: {}",
                    username,
                    agent_id,
                    e.report()
                ),
            }
        }

        // Pass 2: per-user.
        for (username, agent_ids) in &agent_groups {
            match self.synthesize_user(username, agent_ids).await {
                Ok(true) => {
                    users.insert(username.clone());
                }
                Ok(false) => {}
                Err(e) => warn!(
                    "Synthesizer: per-user synthesis for {} failed: {}",
                    username,
                    e.report()
                ),
            }
        }

        // Pass 3: global.
        if !users.is_empty()
            && let Err(e) = self.synthesize_global(&users).await
        {
            warn!("Synthesizer: global synthesis failed: {}", e.report());
        }
    }

    /// Pass 1: fold an agent's changed raw files into its per-agent summary.
    async fn synthesize_agent(
        &self,
        username: &str,
        agent_id: Uuid,
        changed_files: &BTreeSet<String>,
    ) -> Result<bool, MemoryError> {
        let mut sources = Vec::new();
        let mut deleted = Vec::new();

        for file in changed_files {
            match self.storage_read(file).await? {
                Some(content) => sources.push(SourceDoc {
                    name: file.clone(),
                    content,
                }),
                None => deleted.push(file.clone()),
            }
        }
        if sources.is_empty() && deleted.is_empty() {
            return Ok(false);
        }

        if !deleted.is_empty() {
            sources.push(SourceDoc {
                name: "_deleted_sources".to_string(),
                content: format!(
                    "The following memory files were deleted. Remove any content in the running summary that was sourced from them, and do not reference them going forward:\n{}",
                    deleted.join("\n")
                ),
            });
        }

        let target = SynthesisTarget::Agent {
            username: username.to_string(),
            agent_id: agent_id.to_string(),
        };
        let prior_summary = match get_latest_synthesis_file(&self.memory_dir(), &target).await {
            Some(path) => self.storage_read(&path).await?,
            None => None,
        };
        let synthesis = self
            .request_synthesis(target.clone(), prior_summary, sources)
            .await?;

        let summary_path = current_synthesis_path(
            &self.memory_dir(),
            &target,
            Utc::now().date_naive(),
            self.config.synthesis_max_file_bytes,
        )
        .await;
        self.write_synthesis(&summary_path, &synthesis).await?;

        info!(
            "Synthesizer: per-agent synthesis written to {}",
            summary_path
        );

        Ok(true)
    }

    /// Pass 2: fold the changed agents' summaries into the user's summary.
    async fn synthesize_user(
        &self,
        username: &str,
        agent_ids: &BTreeSet<Uuid>,
    ) -> Result<bool, MemoryError> {
        let mut sources = Vec::new();
        for agent_id in agent_ids {
            let target = SynthesisTarget::Agent {
                username: username.to_string(),
                agent_id: agent_id.to_string(),
            };
            let Some(path) = get_latest_synthesis_file(&self.memory_dir(), &target).await else {
                continue;
            };
            if let Some(content) = self.storage_read(&path).await? {
                sources.push(SourceDoc {
                    name: path,
                    content,
                });
            }
        }
        if sources.is_empty() {
            return Ok(false);
        }

        let target = SynthesisTarget::User {
            username: username.to_string(),
        };
        let prior_summary = match get_latest_synthesis_file(&self.memory_dir(), &target).await {
            Some(path) => self.storage_read(&path).await?,
            None => None,
        };
        let synthesis = self
            .request_synthesis(target.clone(), prior_summary, sources)
            .await?;

        let summary_path = current_synthesis_path(
            &self.memory_dir(),
            &target,
            Utc::now().date_naive(),
            self.config.synthesis_max_file_bytes,
        )
        .await;
        self.write_synthesis(&summary_path, &synthesis).await?;

        info!(
            "Synthesizer: per-user synthesis written to {}",
            summary_path
        );

        Ok(true)
    }

    /// Pass 3: fold the changed users' summaries into the global summary.
    async fn synthesize_global(&self, changed_users: &BTreeSet<String>) -> Result<(), MemoryError> {
        let mut sources = Vec::new();
        for user in changed_users {
            let user_target = SynthesisTarget::User {
                username: user.clone(),
            };
            let Some(path) = get_latest_synthesis_file(&self.memory_dir(), &user_target).await
            else {
                continue;
            };
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

        let prior_summary =
            match get_latest_synthesis_file(&self.memory_dir(), &SynthesisTarget::Global).await {
                Some(path) => self.storage_read(&path).await?,
                None => None,
            };
        let synthesis = self
            .request_synthesis(SynthesisTarget::Global, prior_summary, sources)
            .await?;

        let summary_path = current_synthesis_path(
            &self.memory_dir(),
            &SynthesisTarget::Global,
            Utc::now().date_naive(),
            self.config.synthesis_max_file_bytes,
        )
        .await;
        self.write_synthesis(&summary_path, &synthesis).await?;

        info!("Synthesizer: global synthesis written to {}", summary_path);

        Ok(())
    }

    /// Chunk, embed, and write a synthesized document to Storage and Indexer.
    async fn write_synthesis(&self, path: &str, content: &str) -> Result<(), MemoryError> {
        self.storage_write(path, content).await?;

        let text_chunks = chunk_text(content, self.config.chunk_size, self.config.chunk_overlap);
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
            .await?;
        let res = fut.await?;
        Ok(res?)
    }

    async fn storage_write(&self, path: &str, content: &str) -> Result<(), MemoryError> {
        let fut = self
            .storage
            .send(StorageWrite {
                path: path.to_string(),
                content: content.to_string(),
            })
            .await?;
        fut.await??;
        Ok(())
    }

    async fn embed(&self, texts: Vec<String>) -> Result<EmbedResult, MemoryError> {
        let fut = self.llm.send(Embed { texts }).await?;
        let res = fut.await?;
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
            .await?;
        let res = fut.await?;
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
            let fut = self.index.send(EnsureVecReady { dim }).await?;
            fut.await??;
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
            .await?;
        fut.await??;
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
        debug_trace!("Handle command {:?}", msg);

        self.changed_files.insert(msg);

        if !self.timer_armed {
            self.timer_armed = true;

            let addr = ctx.address();
            let cooldown = self.cooldown();
            tokio::spawn(async move {
                tokio::time::sleep(cooldown).await;
                let _ = addr.do_send(CooldownTick).await;
            });
        }
    }
}

impl Handler<CooldownTick> for Synthesizer {
    type Result = ();

    async fn handle(&mut self, _msg: CooldownTick, _ctx: &mut Self::Context) {
        debug_trace!("Handle command {:?}", _msg);

        self.timer_armed = false;
        self.process().await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::task::JoinHandle;

    use super::*;
    use crate::llm::LlmService;
    use crate::memory::{indexer::Indexer, message::StorageWrite, storage::Storage};

    async fn boot() -> (
        Address<Storage>,
        Address<Indexer>,
        Address<LlmService>,
        tempfile::TempDir,
        Vec<JoinHandle<()>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (storage, h1) = Storage::new(PathBuf::from(dir.path())).start("s").unwrap();
        let (index, h2) = Indexer::open_in_memory().unwrap().start("i").unwrap();
        let llm_cfg = crate::llm::LlmConfig {
            provider: "mock".into(),
            embedding_provider: "mock".into(),
            prompts_dir: dir.path().join("prompts"),
            ..Default::default()
        };
        let (llm, h3) = LlmService::new(llm_cfg).start("l").unwrap();
        (storage, index, llm, dir, vec![h1, h2, h3])
    }

    #[tokio::test]
    async fn synthesizer_writes_all_three_tiers() {
        let (storage, index, llm, dir, _handles) = boot().await;

        let agent_id = Uuid::new_v4();
        let rel_path = format!("alice/{}/proj/x.md", agent_id);

        storage
            .send(StorageWrite {
                path: rel_path.clone(),
                content: "hello world".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let cfg = MemoryConfig {
            memory_dir: dir.path().to_string_lossy().to_string(),
            db_path: ":memory:".to_string(),
            synthesizer_cooldown_secs: 0,
            ..MemoryConfig::default()
        };
        let synth = Synthesizer::new(llm, storage.clone(), index, cfg);
        let (addr, _handle) = synth.start("synth").unwrap();

        addr.send(FileChanged {
            username: "alice".to_string(),
            agent_id,
            path: rel_path,
        })
        .await
        .unwrap()
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;

        for folder in [
            dir.path().join(format!("alice/{}/_synthesized", agent_id)),
            dir.path().join("alice/_synthesized"),
            dir.path().join("_synthesized"),
        ] {
            let has_md = std::fs::read_dir(&folder)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
                })
                .unwrap_or(false);
            assert!(has_md, "expected a synthesis file in {:?}", folder);
        }
    }
}
