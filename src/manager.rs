//! Top-level supervisor actor. Owns and supervises [`LlmService`],
//! [`MemoryManager`], and [`HttpServer`].

use std::future::Future;

use acktor::supervisor::{SupervisionEvent, Supervisor};
use acktor::{Actor, ActorContext, Address, Context, ErrorReport, Handler, Recipient, Signal};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, trace, warn};

use crate::config::Config;
use crate::error::ManagerError;
use crate::http::HttpServer;
use crate::llm::{LlmService, provider::build_provider};
use crate::memory::manager::MemoryManager;

pub struct Manager {
    llm: Address<LlmService>,
    memory: Address<MemoryManager>,
    http: Address<HttpServer>,
    llm_handle: Option<JoinHandle<()>>,
    memory_handle: Option<JoinHandle<()>>,
    http_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    shutting_down: bool,
}

impl Manager {
    /// Build the Manager: spawns LLM, then MemoryManager, then HttpServer.
    /// On failure any already-spawned children are dropped (their JoinHandles
    /// are dropped with them; acktor stops them when their mailbox closes).
    pub fn new(config: Config, shutdown_tx: oneshot::Sender<()>) -> Result<Self, ManagerError> {
        let provider = build_provider(&config.llm)?;
        let llm = LlmService::new(config.llm, provider);
        let (llm_addr, llm_handle) = llm
            .start("llm-service")
            .map_err(|e| ManagerError::Actor(format!("LlmService start: {e}")))?;
        info!("LlmService started");

        let memory = MemoryManager::new(config.memory, llm_addr.clone())?;
        let (memory_addr, memory_handle) = memory
            .start("memory-manager")
            .map_err(|e| ManagerError::Actor(format!("MemoryManager start: {e}")))?;
        info!("MemoryManager started");

        let http = HttpServer::new(config.server, memory_addr.clone());
        let (http_addr, http_handle) = http
            .start("http-server")
            .map_err(|e| ManagerError::Actor(format!("HttpServer start: {e}")))?;
        info!("HttpServer started");

        Ok(Self {
            llm: llm_addr,
            memory: memory_addr,
            http: http_addr,
            llm_handle: Some(llm_handle),
            memory_handle: Some(memory_handle),
            http_handle: Some(http_handle),
            shutdown_tx: Some(shutdown_tx),
            shutting_down: false,
        })
    }

    /// Test seam: build a Manager with a caller-supplied LLM provider,
    /// bypassing `build_provider`. Lets tests inject `MockProvider`.
    pub fn new_with_provider(
        config: Config,
        provider: std::sync::Arc<dyn crate::llm::provider::Provider>,
        shutdown_tx: oneshot::Sender<()>,
    ) -> Result<Self, ManagerError> {
        let llm = LlmService::new(config.llm, provider);
        let (llm_addr, llm_handle) = llm
            .start("llm-service")
            .map_err(|e| ManagerError::Actor(format!("LlmService start: {e}")))?;

        let memory = MemoryManager::new(config.memory, llm_addr.clone())?;
        let (memory_addr, memory_handle) = memory
            .start("memory-manager")
            .map_err(|e| ManagerError::Actor(format!("MemoryManager start: {e}")))?;

        let http = HttpServer::new(config.server, memory_addr.clone());
        let (http_addr, http_handle) = http
            .start("http-server")
            .map_err(|e| ManagerError::Actor(format!("HttpServer start: {e}")))?;

        Ok(Self {
            llm: llm_addr,
            memory: memory_addr,
            http: http_addr,
            llm_handle: Some(llm_handle),
            memory_handle: Some(memory_handle),
            http_handle: Some(http_handle),
            shutdown_tx: Some(shutdown_tx),
            shutting_down: false,
        })
    }

    /// Test seam: returns the HttpServer address so tests can simulate child
    /// death by signal-terminating it directly. Not used by production code.
    pub fn http_addr(&self) -> &Address<HttpServer> {
        &self.http
    }
}

impl Manager {
    /// Begins teardown: fire the shutdown oneshot so `main` exits, then
    /// signal-terminate the surviving children. Idempotent — second call
    /// observes `shutting_down` and returns.
    fn initiate_shutdown(&mut self, child: &str) {
        if self.shutting_down {
            debug!("initiate_shutdown ignored (already shutting down), trigger={child}");
            return;
        }
        self.shutting_down = true;
        error!("Manager initiating shutdown after {child} death");

        if let Some(tx) = self.shutdown_tx.take() {
            // Receiver may already be dropped if main exited via ctrl-c; ignore.
            let _ = tx.send(());
        }
    }
}

impl Actor for Manager {
    type Context = Context<Self>;
    type Error = ManagerError;

    async fn post_start(&mut self, ctx: &mut Self::Context) -> Result<(), ManagerError> {
        trace!("Manager post_start: subscribing supervisor events");

        let llm_recipient: Recipient<SupervisionEvent<LlmService>> = ctx.address().into();
        self.llm
            .do_send(Supervisor::Set(llm_recipient))
            .await
            .map_err(|e| ManagerError::Actor(format!("Set LLM supervisor: {e}")))?;

        let memory_recipient: Recipient<SupervisionEvent<MemoryManager>> = ctx.address().into();
        self.memory
            .do_send(Supervisor::Set(memory_recipient))
            .await
            .map_err(|e| ManagerError::Actor(format!("Set MemoryManager supervisor: {e}")))?;

        let http_recipient: Recipient<SupervisionEvent<HttpServer>> = ctx.address().into();
        self.http
            .do_send(Supervisor::Set(http_recipient))
            .await
            .map_err(|e| ManagerError::Actor(format!("Set HttpServer supervisor: {e}")))?;

        info!("Manager is supervising LlmService, MemoryManager, HttpServer");
        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<(), ManagerError> {
        // Drain in reverse startup order: HTTP first (stop accepting work),
        // then MemoryManager (flush in-flight ops), then LLM.
        if let Some(handle) = self.http_handle.take() {
            if let Err(e) = self.http.do_send(Signal::Terminate).await {
                warn!("Could not signal HttpServer: {}", e.report());
                handle.abort();
            }
            if let Err(e) = handle.await {
                warn!("HttpServer join error: {e}");
            }
        }
        if let Some(handle) = self.memory_handle.take() {
            if let Err(e) = self.memory.do_send(Signal::Terminate).await {
                warn!("Could not signal MemoryManager: {}", e.report());
                handle.abort();
            }
            if let Err(e) = handle.await {
                warn!("MemoryManager join error: {e}");
            }
        }
        if let Some(handle) = self.llm_handle.take() {
            if let Err(e) = self.llm.do_send(Signal::Terminate).await {
                warn!("Could not signal LlmService: {}", e.report());
                handle.abort();
            }
            if let Err(e) = handle.await {
                warn!("LlmService join error: {e}");
            }
        }
        info!("Manager is stopped");
        Ok(())
    }
}

impl Handler<SupervisionEvent<LlmService>> for Manager {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<LlmService>,
        _ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        trace!("Manager: SupervisionEvent<LlmService>");
        match msg {
            SupervisionEvent::Warn(_, e) => warn!("LlmService warning: {e}"),
            SupervisionEvent::State(_, s) => debug!("LlmService state: {s:?}"),
            SupervisionEvent::Terminated(_, e) => {
                error!("LlmService terminated: {e:?}");
                self.initiate_shutdown("LlmService");
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("LlmService panicked: {info}");
                self.initiate_shutdown("LlmService");
            }
        }
        std::future::ready(())
    }
}

impl Handler<SupervisionEvent<MemoryManager>> for Manager {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<MemoryManager>,
        _ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        trace!("Manager: SupervisionEvent<MemoryManager>");
        match msg {
            SupervisionEvent::Warn(_, e) => warn!("MemoryManager warning: {e}"),
            SupervisionEvent::State(_, s) => debug!("MemoryManager state: {s:?}"),
            SupervisionEvent::Terminated(_, e) => {
                error!("MemoryManager terminated: {e:?}");
                self.initiate_shutdown("MemoryManager");
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("MemoryManager panicked: {info}");
                self.initiate_shutdown("MemoryManager");
            }
        }
        std::future::ready(())
    }
}

impl Handler<SupervisionEvent<HttpServer>> for Manager {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<HttpServer>,
        _ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        trace!("Manager: SupervisionEvent<HttpServer>");
        match msg {
            SupervisionEvent::Warn(_, e) => warn!("HttpServer warning: {e}"),
            SupervisionEvent::State(_, s) => debug!("HttpServer state: {s:?}"),
            SupervisionEvent::Terminated(_, e) => {
                error!("HttpServer terminated: {e:?}");
                self.initiate_shutdown("HttpServer");
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("HttpServer panicked: {info}");
                self.initiate_shutdown("HttpServer");
            }
        }
        std::future::ready(())
    }
}
