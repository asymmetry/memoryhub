use std::time::Duration;

use acktor::{Actor, Signal};
use anyhow::Result;
use tokio::sync::oneshot;
use tracing::{info, warn};

use clawchorus::{config, manager::Manager};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::Config::load()?;
    info!(
        host = %config.server.host,
        port = config.server.port,
        "ClawChorus starting"
    );
    info!(
        provider = %config.llm.provider,
        model = %config.llm.model,
        "LLM configuration"
    );

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let manager = Manager::new(config, shutdown_tx)?;
    let (manager_addr, manager_handle) = manager.start("manager")?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("ctrl-c received, shutting down");
            if let Err(e) = manager_addr.do_send(Signal::Terminate).await {
                warn!("Could not signal Manager: {e}");
            }
        }
        _ = shutdown_rx => {
            warn!("Manager initiated shutdown after child failure");
            // Manager has already begun teardown; sending Terminate again is harmless.
            let _ = manager_addr.do_send(Signal::Terminate).await;
        }
    }

    match tokio::time::timeout(Duration::from_secs(5), manager_handle).await {
        Ok(Ok(_)) => info!("Manager stopped cleanly"),
        Ok(Err(e)) => warn!("Manager join error: {e}"),
        Err(_) => warn!("Manager did not stop within 5s; exiting anyway"),
    }

    Ok(())
}
