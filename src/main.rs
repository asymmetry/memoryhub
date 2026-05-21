use std::time::Duration;

use acktor::{Actor, ErrorReport, Signal};
use anyhow::Result;
use tracing::{info, warn};

use memoryhub::{MemoryHub, config};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::Config::load().await?;
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
