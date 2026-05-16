//! HTTP service: Axum-based JSON API over the Memory Manager actor.

pub mod dto;
pub mod error;
pub mod handlers;
pub mod router;

use std::io;
use std::net::SocketAddr;

use acktor::{Actor, Address, Context};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{info, trace};

use crate::config::ServerConfig;
use crate::http::router::build_router;
use crate::memory::MemoryManager;

#[derive(Debug, Error)]
pub enum HttpServerError {
    #[error("bind error on {addr}: {source}")]
    Bind { addr: String, source: io::Error },
    #[error("invalid bind address {addr}: {source}")]
    InvalidAddr {
        addr: String,
        source: std::net::AddrParseError,
    },
}

pub struct HttpServer {
    config: ServerConfig,
    memory_manager: Address<MemoryManager>,
    serve_handle: Option<JoinHandle<()>>,
}

impl HttpServer {
    pub fn new(config: ServerConfig, memory_manager: Address<MemoryManager>) -> Self {
        Self {
            config,
            memory_manager,
            serve_handle: None,
        }
    }
}

impl Actor for HttpServer {
    type Context = Context<Self>;
    type Error = HttpServerError;

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<(), HttpServerError> {
        trace!("HttpServer post_start");
        let addr_str = format!("{}:{}", self.config.host, self.config.port);
        let addr: SocketAddr = addr_str
            .parse()
            .map_err(|source| HttpServerError::InvalidAddr {
                addr: addr_str.clone(),
                source,
            })?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| HttpServerError::Bind {
                addr: addr_str.clone(),
                source,
            })?;
        let app = build_router(self.memory_manager.clone());
        info!(%addr, "HttpServer listening");
        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("axum serve error: {e}");
            }
        });
        self.serve_handle = Some(handle);
        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<(), HttpServerError> {
        if let Some(handle) = self.serve_handle.take() {
            handle.abort();
        }
        info!("HttpServer is stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmService;
    use crate::llm::provider::mock::MockProvider;
    use crate::memory::config::MemoryConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn http_server_starts_and_stops() {
        let provider = Arc::new(MockProvider::new());
        let llm = LlmService::new(Default::default(), provider);
        let (llm_addr, _llm_handle) = llm.start("llm-test").unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mm_cfg = MemoryConfig {
            memory_dir: dir.path().to_string_lossy().to_string(),
            db_path: ":memory:".to_string(),
            ..MemoryConfig::default()
        };
        let mm = MemoryManager::new(mm_cfg, llm_addr).unwrap();
        let (mm_addr, _mm_handle) = mm.start("memory-manager").unwrap();

        // Pick an ephemeral port by binding 0.
        let cfg = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
        };
        let server = HttpServer::new(cfg, mm_addr);
        let (server_addr, server_handle) = server.start("http-server").unwrap();

        // Stop the actor cleanly.
        server_addr
            .do_send(acktor::Signal::Terminate)
            .await
            .unwrap();
        let _ = server_handle.await;
    }
}
