//! MemoryHub MCP server: a stdio MCP server exposing MemoryHub API as tools.

mod client;
mod config;
mod identity;
mod server;

use rmcp::{ServiceExt, transport::stdio};

use crate::client::MemoryHubClient;
use crate::config::Config;
use crate::server::McpServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("memoryhub-mcp: {}", e);
            std::process::exit(1);
        }
    };

    let config_dir = dirs::config_dir()
        .map(|d| d.join("memoryhub"))
        .unwrap_or_else(|| std::path::PathBuf::from(".memoryhub"));
    let client = MemoryHubClient::new(config.url.clone(), config.token.clone());
    let server = McpServer::new(client, config, config_dir);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
