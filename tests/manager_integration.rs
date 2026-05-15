use std::sync::Arc;
use std::time::Duration;

use acktor::{Actor, Signal};
use clawchorus::config::{Config, MemoryConfig, ServerConfig};
use clawchorus::llm::provider::mock::MockProvider;
use clawchorus::manager::Manager;
use tokio::sync::oneshot;

fn test_config(dir: &std::path::Path) -> Config {
    Config {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // ephemeral
        },
        memory: MemoryConfig {
            memory_dir: dir.to_string_lossy().to_string(),
            db_path: ":memory:".to_string(),
            ..MemoryConfig::default()
        },
        ..Config::default()
    }
}

#[tokio::test]
async fn manager_starts_and_stops_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new());
    let (shutdown_tx, _shutdown_rx) = oneshot::channel();

    let manager = Manager::new_with_provider(test_config(dir.path()), provider, shutdown_tx)
        .expect("Manager::new_with_provider");
    let (addr, handle) = manager.start("manager").unwrap();

    // Let post_start subscribe supervisors.
    tokio::time::sleep(Duration::from_millis(100)).await;

    addr.do_send(Signal::Terminate).await.unwrap();

    let join = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(join.is_ok(), "Manager did not stop within 5s");
    assert!(join.unwrap().is_ok(), "Manager join returned error");
}

#[tokio::test]
async fn manager_initiates_shutdown_when_child_dies() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let manager = Manager::new_with_provider(test_config(dir.path()), provider, shutdown_tx)
        .expect("Manager::new_with_provider");

    // Grab a clone of HttpServer's address BEFORE start consumes self.
    let http_addr = manager.http_addr().clone();
    let (_manager_addr, manager_handle) = manager.start("manager").unwrap();

    // Allow post_start to subscribe supervisors.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Kill HttpServer.
    http_addr.do_send(Signal::Terminate).await.unwrap();

    // Manager should see SupervisionEvent::Terminated and fire shutdown_rx.
    let shutdown = tokio::time::timeout(Duration::from_secs(2), shutdown_rx).await;
    assert!(
        shutdown.is_ok() && shutdown.unwrap().is_ok(),
        "shutdown_rx did not fire after HttpServer death"
    );

    // Manager itself should also terminate cleanly within a generous window.
    let _ = tokio::time::timeout(Duration::from_secs(5), manager_handle).await;
}
