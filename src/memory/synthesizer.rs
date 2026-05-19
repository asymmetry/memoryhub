//! Synthesizer Actor — long-lived child of MemoryManager.
//!
//! Receives fire-and-forget `FileChanged` notifications, batches them with a
//! cool-down timer, then runs a synthesis pipeline: read source files,
//! converse with LlmService via a Session, chunk + embed the synthesized
//! result, and write it back via Storage + Index under a `_synthesized/`
//! path.

use std::collections::BTreeSet;
use std::time::Duration;

use acktor::{Actor, ActorContext, Address, Context, Handler, Message};
use tokio::time::Instant;
use tracing::{error, info, trace, warn};

use crate::llm::session::{SendMessage, StopSession};
use crate::llm::{Embed, LlmService, StartSession};
use crate::memory::{
    MemoryType,
    chunking::chunk_text,
    error::MemoryError,
    index::Index,
    messages::{Chunk, EnsureVecReady, FileChanged, IndexInsert, StorageRead, StorageWrite},
    path::derive_synthesis_path,
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

    async fn process(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let paths: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
        info!(count = paths.len(), "Synthesizer: processing pending set");

        let mut sources = Vec::new();
        for path in &paths {
            match self.storage.send(StorageRead { path: path.clone() }).await {
                Ok(fut) => match fut.await {
                    Ok(Ok(Some(content))) => sources.push((path.clone(), content)),
                    Ok(Ok(None)) => trace!(path = %path, "Synthesizer: source vanished, skipping"),
                    Ok(Err(e)) => warn!(path = %path, error = %e, "Synthesizer: read failed"),
                    Err(e) => warn!(path = %path, error = %e, "Synthesizer: read join failed"),
                },
                Err(e) => warn!(path = %path, error = %e, "Synthesizer: read send failed"),
            }
        }
        if sources.is_empty() {
            return;
        }

        let session_addr = match self.llm.send(StartSession).await {
            Ok(fut) => match fut.await {
                Ok(Ok(addr)) => addr,
                Ok(Err(e)) => {
                    error!("Synthesizer: StartSession failed: {}", e);
                    return;
                }
                Err(e) => {
                    error!("Synthesizer: StartSession join failed: {}", e);
                    return;
                }
            },
            Err(e) => {
                error!("Synthesizer: StartSession send failed: {}", e);
                return;
            }
        };

        let prompt = build_synthesis_prompt(&sources);
        let synthesis = match session_addr.send(SendMessage { content: prompt }).await {
            Ok(fut) => match fut.await {
                Ok(Ok(reply)) => reply,
                Ok(Err(e)) => {
                    error!("Synthesizer: SendMessage failed: {}", e);
                    let _ = session_addr.send(StopSession).await;
                    return;
                }
                Err(e) => {
                    error!("Synthesizer: SendMessage join failed: {}", e);
                    return;
                }
            },
            Err(e) => {
                error!("Synthesizer: SendMessage send failed: {}", e);
                return;
            }
        };
        let _ = session_addr.send(StopSession).await;

        let text_chunks = chunk_text(&synthesis, self.chunk_size, self.chunk_overlap);
        if text_chunks.is_empty() {
            return;
        }
        let texts: Vec<String> = text_chunks.iter().map(|c| c.text.clone()).collect();
        let embed_result = match self.llm.send(Embed { texts }).await {
            Ok(fut) => match fut.await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    error!("Synthesizer: Embed failed: {}", e);
                    return;
                }
                Err(e) => {
                    error!("Synthesizer: Embed join failed: {}", e);
                    return;
                }
            },
            Err(e) => {
                error!("Synthesizer: Embed send failed: {}", e);
                return;
            }
        };

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

        let common_user = common_username(&sources);
        let synth_path = derive_synthesis_path(
            common_user.as_deref(),
            MemoryType::LongTerm,
            &synthesis_filename(),
        );

        match self
            .storage
            .send(StorageWrite {
                path: synth_path.clone(),
                content: synthesis.clone(),
            })
            .await
        {
            Ok(fut) => {
                if let Ok(Err(e)) = fut.await {
                    error!(path = %synth_path, error = %e, "Synthesizer: storage write failed");
                    return;
                }
            }
            Err(e) => {
                error!("Synthesizer: storage send failed: {}", e);
                return;
            }
        }

        if let Some(first) = chunks.first() {
            let dim = first.embedding.0.len();
            if let Ok(fut) = self.index.send(EnsureVecReady { dim }).await {
                if let Ok(Err(e)) = fut.await {
                    error!("Synthesizer: EnsureVecReady failed: {}", e);
                    return;
                }
            }
        }
        match self
            .index
            .send(IndexInsert {
                path: synth_path.clone(),
                source: "synthesized".to_string(),
                size: synthesis.len() as u64,
                model: embed_result.model,
                chunks,
            })
            .await
        {
            Ok(fut) => {
                if let Ok(Err(e)) = fut.await {
                    error!(path = %synth_path, error = %e, "Synthesizer: index insert failed");
                }
            }
            Err(e) => error!("Synthesizer: index send failed: {}", e),
        }

        info!(path = %synth_path, "Synthesizer: synthesis written");
    }
}

fn build_synthesis_prompt(sources: &[(String, String)]) -> String {
    let mut out = String::from("Synthesize the following memories:\n\n");
    for (path, content) in sources {
        out.push_str(&format!("## {}\n{}\n\n", path, content));
    }
    out
}

fn common_username(sources: &[(String, String)]) -> Option<String> {
    let first = sources.first()?.0.split('/').next()?.to_string();
    if first == "_synthesized" {
        return None;
    }
    for (path, _) in sources {
        match path.split('/').next() {
            Some(u) if u == first => continue,
            _ => return None,
        }
    }
    Some(first)
}

fn synthesis_filename() -> String {
    format!(
        "synthesis-{}.md",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

impl Actor for Synthesizer {
    type Context = Context<Self>;
    type Error = MemoryError;
}

impl Handler<FileChanged> for Synthesizer {
    type Result = ();

    async fn handle(&mut self, msg: FileChanged, ctx: &mut Self::Context) {
        trace!(rel_path = %msg.rel_path, "Synthesizer: FileChanged");
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

    async fn handle(&mut self, _msg: CooldownTick, _ctx: &mut Self::Context) {
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
        let provider = std::sync::Arc::new(crate::llm::provider::mock::MockProvider::new());
        let (llm, h3) = LlmService::new(Default::default(), provider)
            .start("l")
            .unwrap();
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

        let target_dir = dir
            .path()
            .join("alice")
            .join("_synthesized")
            .join("long_term");
        let entries: Vec<_> = std::fs::read_dir(&target_dir)
            .map(|rd| rd.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(
            !entries.is_empty(),
            "expected synthesis file under {:?}",
            target_dir
        );
    }

    #[test]
    fn common_username_detects_single_owner() {
        let sources = vec![
            ("alice/a/daily_note/x.md".to_string(), String::new()),
            ("alice/b/long_term/y.md".to_string(), String::new()),
        ];
        assert_eq!(common_username(&sources).as_deref(), Some("alice"));
    }

    #[test]
    fn common_username_returns_none_for_mixed_owners() {
        let sources = vec![
            ("alice/a/daily_note/x.md".to_string(), String::new()),
            ("bob/b/long_term/y.md".to_string(), String::new()),
        ];
        assert!(common_username(&sources).is_none());
    }
}
