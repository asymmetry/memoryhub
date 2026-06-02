//! MemoryHub MCP server: a stdio MCP server exposing MemoryHub API as tools.

use std::path::PathBuf;

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};

use memoryhub_mcp::{client::MemoryHubClient, config::Config, server::McpServer};

#[tokio::main]
async fn main() -> Result<()> {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("memoryhub-mcp: {}", e);
            std::process::exit(1);
        }
    };

    let config_dir = dirs::config_dir()
        .map(|d| d.join("memoryhub"))
        .unwrap_or_else(|| PathBuf::from(".memoryhub"));
    let client = MemoryHubClient::new(config.url.clone(), config.token.clone());
    let server = McpServer::new(client, config, config_dir);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
