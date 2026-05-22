use std::time::Duration;

use acktor::{Actor, ErrorReport, Signal};
use anyhow::Result;
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use memoryhub::{MemoryHub, config};

mod cli;
use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = match &cli.log_level {
        Some(level) => EnvFilter::try_new(level)?,
        None => EnvFilter::from_default_env(),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let base_dir = config::base_dir(cli.base_dir.as_deref())?;
    info!("Using base directory {}", base_dir.display());

    let mut config = config::Config::load(cli.config.clone(), &base_dir).await?;
    cli.apply_overrides(&mut config);
    info!(
        "MemoryHub starting on {}:{}",
        config.server.host, config.server.port
    );
    info!(
        "Using LLM provider '{}' with model '{}'",
        config.llm.provider, config.llm.model
    );

    let manager = MemoryHub::new(config);
    let (manager_addr, mut manager_handle) = manager.start("mgr")?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl-c received, stopping MemoryHub...");

            if let Err(e) = manager_addr.do_send(Signal::Stop).await {
                warn!("Could not signal MemoryHub: {}", e.report());
            }

            tokio::time::timeout(Duration::from_secs(5), manager_handle).await??;

            info!("MemoryHub stopped");

            Ok(())
        }
        res = &mut manager_handle => {
            warn!("MemoryHub stopped unexpectedly");

            Ok(res?)
        }
    }
}
