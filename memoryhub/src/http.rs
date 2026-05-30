//! HTTP service: Axum-based JSON API over the Memory Manager actor.

use std::net::SocketAddr;
use std::sync::Arc;

use acktor::{Actor, Address, Context, ErrorReport, JoinHandle};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::{Instrument, error, info, warn};

use crate::memory::MemoryManager;

mod router;
pub use router::build_router;

pub mod error;
pub use error::{AuthError, HttpServerError};

pub mod auth;
pub use auth::{AuthConfig, AuthStore, NewToken, Principal, TokenInfo, UserInfo};

pub mod middleware;
pub use middleware::{AdminPrincipal, AuthUser, auth_middleware};

pub mod admin;

/// HTTP server bind settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8000,
        }
    }
}

/// State shared across all HTTP handlers.
///
/// Holds the dependencies needed to service requests.
#[derive(Clone)]
pub struct HttpServerState {
    /// Address of the Memory Manager actor that handlers dispatch requests to.
    pub memory_manager: Address<MemoryManager>,
    /// User + token storage backing authentication.
    pub auth: Arc<AuthStore>,
}

impl HttpServerState {
    /// Constructs the shared state of the `HttpServer`.
    pub fn new(memory_manager: Address<MemoryManager>, auth: Arc<AuthStore>) -> Self {
        Self {
            memory_manager,
            auth,
        }
    }
}

/// Actor that owns the Axum HTTP server.
pub struct HttpServer {
    config: ServerConfig,
    auth_config: AuthConfig,
    memory_manager: Address<MemoryManager>,
    serve_handle: Option<JoinHandle<()>>,
}

impl HttpServer {
    /// Constructs a new `HttpServer`.
    pub fn new(
        config: ServerConfig,
        auth_config: AuthConfig,
        memory_manager: Address<MemoryManager>,
    ) -> Self {
        Self {
            config,
            auth_config,
            memory_manager,
            serve_handle: None,
        }
    }
}

impl Actor for HttpServer {
    type Context = Context<Self>;
    type Error = HttpServerError;

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<(), HttpServerError> {
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

        let auth_store = if self.auth_config.db_path == ":memory:" {
            AuthStore::open_in_memory(self.auth_config.admin_token.clone())?
        } else {
            AuthStore::open(
                std::path::Path::new(&self.auth_config.db_path),
                self.auth_config.admin_token.clone(),
            )?
        };

        if self.auth_config.admin_token.is_none() && !auth_store.has_admin().await.unwrap_or(false)
        {
            warn!(
                "User/token management is unreachable: no root admin token configured and no \
                 admin user exists"
            );
        }

        let auth_store = Arc::new(auth_store);
        let app = build_router(HttpServerState::new(
            self.memory_manager.clone(),
            auth_store,
        ));

        let handle = tokio::spawn(
            async move {
                if let Err(e) = axum::serve(listener, app).await {
                    error!("Could not start HTTP server: {}", e.report());
                }
            }
            .in_current_span(),
        );
        self.serve_handle = Some(handle);

        info!("HttpServer is listening on {}", addr);

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
