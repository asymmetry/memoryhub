use std::future::Future;

use acktor::{
    Actor, ActorContext, Address, Context, ErrorReport, Handler,
    supervisor::SupervisionEvent,
    utils::{debug_trace, terminate_actor},
};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::error::MemoryHubError;
use crate::http::HttpServer;
use crate::llm::LlmService;
use crate::memory::MemoryManager;

pub struct MemoryHub {
    config: Config,
    llm: Option<Address<LlmService>>,
    memory: Option<Address<MemoryManager>>,
    http: Option<Address<HttpServer>>,
    llm_handle: Option<JoinHandle<()>>,
    memory_handle: Option<JoinHandle<()>>,
    http_handle: Option<JoinHandle<()>>,
}

impl MemoryHub {
    /// Constructs the a new `MemoryHub` instance.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            llm: None,
            memory: None,
            http: None,
            llm_handle: None,
            memory_handle: None,
            http_handle: None,
        }
    }
}

impl Actor for MemoryHub {
    type Context = Context<Self>;
    type Error = MemoryHubError;

    fn pre_start(&mut self, ctx: &mut Self::Context) -> Result<(), MemoryHubError> {
        let Config {
            server,
            memory,
            llm,
            auth,
        } = self.config.clone();

        let (llm_addr, llm_handle) = LlmService::create("llm-service", |child_ctx| {
            child_ctx.set_supervisor(Some(ctx.address().into()));
            Ok(LlmService::new(llm))
        })?;

        let (memory_addr, memory_handle) = MemoryManager::create("memory-manager", |child_ctx| {
            child_ctx.set_supervisor(Some(ctx.address().into()));
            Ok(MemoryManager::new(memory, llm_addr.clone()))
        })?;

        let (http_addr, http_handle) = HttpServer::create("http-server", |child_ctx| {
            child_ctx.set_supervisor(Some(ctx.address().into()));
            Ok(HttpServer::new(server, auth.clone(), memory_addr.clone()))
        })?;

        self.llm = Some(llm_addr);
        self.memory = Some(memory_addr);
        self.http = Some(http_addr);
        self.llm_handle = Some(llm_handle);
        self.memory_handle = Some(memory_handle);
        self.http_handle = Some(http_handle);

        Ok(())
    }

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<(), MemoryHubError> {
        info!("MemoryHub is ready");

        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<(), MemoryHubError> {
        // Drain in reverse startup order: HTTP first (stop accepting work),
        // then MemoryManager (flush in-flight ops), then LLM.
        if let (Some(addr), Some(handle)) = (self.http.take(), self.http_handle.take()) {
            terminate_actor(addr, handle).await;
        }

        if let (Some(addr), Some(handle)) = (self.memory.take(), self.memory_handle.take()) {
            terminate_actor(addr, handle).await;
        }

        if let (Some(addr), Some(handle)) = (self.llm.take(), self.llm_handle.take()) {
            terminate_actor(addr, handle).await;
        }

        Ok(())
    }
}

impl Handler<SupervisionEvent<MemoryManager>> for MemoryHub {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<MemoryManager>,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        debug_trace!("Handling supervision event {:?}", msg);

        match msg {
            SupervisionEvent::Warn(_, e) => warn!("MemoryManager warning: {}", e.report()),
            SupervisionEvent::Terminated(_, e) => {
                match e {
                    Some(e) => error!("MemoryManager terminated with error: {}", e.report()),
                    None => debug!("MemoryManager terminated"),
                }
                ctx.stop();
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("MemoryManager panicked: {}", info);
                ctx.stop();
            }
            _ => {}
        }

        std::future::ready(())
    }
}

impl Handler<SupervisionEvent<LlmService>> for MemoryHub {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<LlmService>,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        debug_trace!("Handling supervision event {:?}", msg);

        match msg {
            SupervisionEvent::Warn(_, e) => warn!("LlmService warning: {}", e.report()),
            SupervisionEvent::Terminated(_, e) => {
                match e {
                    Some(e) => error!("LlmService terminated with error: {}", e.report()),
                    None => debug!("LlmService terminated"),
                }
                ctx.stop();
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("LlmService panicked: {}", info);
                ctx.stop();
            }
            _ => {}
        }

        std::future::ready(())
    }
}

impl Handler<SupervisionEvent<HttpServer>> for MemoryHub {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<HttpServer>,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        debug_trace!("Handling supervision event {:?}", msg);

        match msg {
            SupervisionEvent::Warn(_, e) => warn!("HttpServer warning: {}", e.report()),
            SupervisionEvent::Terminated(_, e) => {
                match e {
                    Some(e) => error!("HttpServer terminated with error: {}", e.report()),
                    None => debug!("HttpServer terminated"),
                }
                ctx.stop();
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("HttpServer panicked: {}", info);
                ctx.stop();
            }
            _ => {}
        }

        std::future::ready(())
    }
}
