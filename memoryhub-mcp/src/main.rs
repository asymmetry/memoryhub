//! MemoryHub MCP server + hook-CLI: stdio MCP by default, `upload`/`recall`/`config` subcommands.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use uuid::Uuid;

use memoryhub_mcp::{
    client::HttpClient,
    config::{Config, FileConfig},
    identity,
    server::McpServer,
};

mod cli;
use cli::{Cli, Command};

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("memoryhub"))
        .unwrap_or_else(|| PathBuf::from(".memoryhub"))
}

fn create_http_client(agent: &str) -> Result<(HttpClient, Uuid)> {
    let dir = config_dir();
    let config = Config::from_file(&dir.join("config.toml"))?;
    let agent_id = identity::resolve_agent_id(config.agent_id, Some(agent), &dir)?;

    Ok((HttpClient::new(config.url, config.token), agent_id))
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        None => launch_mcp_server().await,

        Some(Command::Config { check }) => {
            if check {
                check_config()
            } else {
                run_config()
            }
        }

        Some(Command::Upload {
            agent,
            project,
            filename,
            path,
        }) => {
            let (client, agent_id) = create_http_client(&agent)?;
            let items = match (filename, path) {
                (Some(filename), Some(path)) => vec![cli::UploadItem {
                    project,
                    filename,
                    path,
                }],
                _ => cli::parse_items(io::stdin().lock())?,
            };
            for (name, err) in cli::upload_items(&client, agent_id, items).await {
                eprintln!("memoryhub-mcp: upload {} failed: {}", name, err);
            }

            Ok(())
        }

        Some(Command::Recall { agent, scope }) => {
            let (client, agent_id) = create_http_client(&agent)?;
            match client.summary(Some(agent_id), &scope).await {
                Ok(Some(text)) => {
                    print!("{text}");
                    io::stdout().flush().ok();
                }
                Ok(None) => {}
                Err(e) => eprintln!("memoryhub-mcp: recall failed: {}", e),
            }
            Ok(())
        }
    }
}

async fn launch_mcp_server() -> Result<()> {
    let config = match Config::from_envs() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("memoryhub-mcp: {e}");
            std::process::exit(1);
        }
    };

    let client = HttpClient::new(config.url.clone(), config.token.clone());
    let server = McpServer::new(client, config, config_dir());

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}

/// Exit 0 if a usable connection config resolves, non-zero otherwise. Quiet (no prompts,
/// no error text) — it's meant for a plugin hook to test whether setup has been done.
fn check_config() -> Result<()> {
    if Config::from_file(&config_dir().join("config.toml")).is_err() {
        std::process::exit(1);
    }
    Ok(())
}

fn run_config() -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir).ok();

    let prompt = |label: &str| -> Result<String> {
        print!("{label}: ");
        io::stdout().flush().ok();
        let mut s = String::new();
        io::stdin().read_line(&mut s)?;
        Ok(s.trim().to_string())
    };

    let url = prompt("MemoryHub URL")?;
    let token = prompt("MemoryHub token (mh_...)")?;

    let body = toml::to_string(&FileConfig {
        url: Some(url),
        token: Some(token),
    })?;
    let path = dir.join("config.toml");
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;

    println!("Saved {}", path.display());

    Ok(())
}
