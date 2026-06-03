//! MemoryHub MCP server + hook-CLI: stdio MCP by default, `upload`/`recall`/`config` subcommands.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::stdio};

use memoryhub_mcp::{cli, client::MemoryHubClient, config::Config, identity, server::McpServer};

#[derive(Parser)]
#[command(name = "memoryhub-mcp")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Upload memory files (stdin JSON array, or a single --filename/--path).
    Upload {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        filename: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
    /// Print the latest synthesized summary for the agent's scope.
    Recall {
        #[arg(long)]
        agent: String,
        #[arg(long, default_value = "user")]
        scope: String,
    },
    /// Write url/token to <config_dir>/memoryhub/config.json (interactive).
    Config,
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("memoryhub"))
        .unwrap_or_else(|| PathBuf::from(".memoryhub"))
}

fn cli_client(agent: &str) -> Result<(MemoryHubClient, uuid::Uuid)> {
    let dir = config_dir();
    let (url, token) = Config::load_connection(
        &dir,
        std::env::var("MEMORYHUB_URL").ok(),
        std::env::var("MEMORYHUB_TOKEN").ok(),
    )?;
    let agent_id_override = std::env::var("MEMORYHUB_AGENT_ID")
        .ok()
        .and_then(|s| s.parse().ok());
    let agent_id = identity::resolve_agent_id(agent_id_override, Some(agent), &dir)?;
    Ok((MemoryHubClient::new(url, token), agent_id))
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        None => run_server().await,
        Some(Command::Upload {
            agent,
            project,
            filename,
            path,
        }) => {
            let (client, agent_id) = cli_client(&agent)?;
            let items = match (filename, path) {
                (Some(filename), Some(path)) => vec![cli::UploadItem {
                    project,
                    filename,
                    path,
                }],
                _ => cli::parse_items(std::io::stdin().lock())?,
            };
            for (name, err) in cli::upload_items(&client, agent_id, items).await {
                eprintln!("memoryhub-mcp: upload {name} failed: {err}");
            }
            Ok(())
        }
        Some(Command::Recall { agent, scope }) => {
            let (client, agent_id) = cli_client(&agent)?;
            match client.summary(Some(agent_id), &scope).await {
                Ok(Some(text)) => {
                    print!("{text}");
                    std::io::stdout().flush().ok();
                }
                Ok(None) => {}
                Err(e) => eprintln!("memoryhub-mcp: recall failed: {e}"),
            }
            Ok(())
        }
        Some(Command::Config) => run_config(),
    }
}

async fn run_server() -> Result<()> {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("memoryhub-mcp: {e}");
            std::process::exit(1);
        }
    };
    let client = MemoryHubClient::new(config.url.clone(), config.token.clone());
    let server = McpServer::new(client, config, config_dir());
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn run_config() -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).ok();
    let prompt = |label: &str| -> Result<String> {
        print!("{label}: ");
        std::io::stdout().flush().ok();
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        Ok(s.trim().to_string())
    };
    let url = prompt("MemoryHub URL")?;
    let token = prompt("MemoryHub token (mh_...)")?;
    let body = serde_json::json!({ "url": url, "token": token });
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_string_pretty(&body)?,
    )
    .context("writing config.json")?;
    println!("Saved {}", dir.join("config.json").display());
    Ok(())
}
