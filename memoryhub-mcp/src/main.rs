//! MemoryHub MCP server: a stdio MCP server exposing MemoryHub memories as tools.

mod client;
mod config;
mod identity;
mod server;

use rmcp::{ServiceExt, transport::stdio};

use crate::client::MemoryClient;
use crate::config::Config;
use crate::server::MemoryServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("memoryhub-mcp: {}", e);
            std::process::exit(1);
        }
    };

    let config_dir = config_dir();
    let client = MemoryClient::new(config.url.clone(), config.token.clone());
    let server = MemoryServer::new(client, config, config_dir);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// `$XDG_CONFIG_HOME/memoryhub` or `~/.config/memoryhub`, falling back to the current directory.
fn config_dir() -> std::path::PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return std::path::PathBuf::from(xdg).join("memoryhub");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return std::path::PathBuf::from(home)
            .join(".config")
            .join("memoryhub");
    }
    std::path::PathBuf::from(".memoryhub")
}
