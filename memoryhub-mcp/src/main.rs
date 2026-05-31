//! MemoryHub MCP server: a stdio MCP server exposing MemoryHub memories as tools.

mod client;
mod config;
mod identity;
mod server;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{ServerHandler, ServiceExt, model::*, tool_handler, tool_router, transport::stdio};

#[derive(Clone)]
struct MemoryServer {
    tool_router: ToolRouter<MemoryServer>,
}

#[tool_router]
impl MemoryServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some("MemoryHub memory tools (coming online).".to_string());
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = MemoryServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
