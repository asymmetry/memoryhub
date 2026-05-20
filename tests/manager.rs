use std::time::Duration;

use acktor::{Actor, Signal};
use clawchorus::{
    ClawChorus,
    config::{Config, LlmConfig, MemoryConfig, ServerConfig},
};

/// A config that builds the mock LLM provider (via the `_test` feature, which
/// `cargo test` enables) and keeps everything else in-memory and ephemeral.
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
        llm: LlmConfig {
            provider: "mock".to_string(),
            embedding_provider: "mock".to_string(),
            ..LlmConfig::default()
        },
        ..Config::default()
    }
}

#[tokio::test]
async fn manager_starts_and_stops_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ClawChorus::new(test_config(dir.path()));
    let (addr, handle) = manager.start("manager").unwrap();

    // Let pre_start spawn the children.
    tokio::time::sleep(Duration::from_millis(100)).await;

    addr.do_send(Signal::Stop).await.unwrap();

    let join = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(join.is_ok(), "Manager did not stop within 5s");
    assert!(join.unwrap().is_ok(), "Manager join returned error");
}

#[tokio::test]
async fn manager_stops_itself_when_child_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    // An unparseable bind address makes HttpServer fail in its `post_start`,
    // which terminates it and notifies the Manager (its supervisor).
    config.server.host = "not-an-ip-address".to_string();

    let manager = ClawChorus::new(config);
    let (_addr, handle) = manager.start("manager").unwrap();

    // HttpServer fails async; the Manager should see SupervisionEvent::Terminated,
    // call ctx.stop(), run post_stop, and resolve its own JoinHandle.
    let join = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        join.is_ok(),
        "Manager did not stop itself after HttpServer failure"
    );
    assert!(join.unwrap().is_ok(), "Manager join returned error");
}
