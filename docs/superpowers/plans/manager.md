# Manager Actor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a top-level `Manager` actor that owns `LlmService`, `MemoryManager`, and `HttpServer`, subscribes to their supervision events, and initiates a clean process-wide shutdown when any child dies.

**Architecture:** A single Manager actor at `src/manager.rs`. On construction it spawns the three children (LLM → MemoryManager → HttpServer). On `post_start` it subscribes itself as supervisor of each. It handles `SupervisionEvent<X>` for each child: `Warn`/`State` are logged; `Terminated`/`Panicked` trigger `initiate_shutdown` which fires a `oneshot::Sender<()>` that `main` is selecting on alongside `ctrl_c`. No restart logic.

**Tech Stack:** Rust 2024, acktor 1.1 (`Supervisor`, `SupervisionEvent`, `Recipient`), tokio (`oneshot`, `signal`, `time::timeout`), thiserror, tracing.

**Spec:** `docs/superpowers/specs/manager-design.md`

---

## File Structure

```
src/
  manager.rs           // Manager actor (~150 LOC)
  error.rs             // add ManagerError
  lib.rs               // add `pub mod manager;`
  main.rs              // simplified: only Manager + shutdown_rx + select!
tests/
  manager_integration.rs   // two integration tests
```

---

## Task 1: ManagerError

**Files:**
- Modify: `src/error.rs`

- [ ] **Step 1: Inspect existing error.rs**

Read `src/error.rs`. Today it defines `ConfigError`. We add `ManagerError` alongside it. Do not touch `ConfigError`.

- [ ] **Step 2: Edit `src/error.rs`**

Append the following at the end of `src/error.rs` (after the existing `ConfigError` definition):

```rust
use crate::http::HttpServerError;
use crate::llm::LlmError;
use crate::memory::error::MemoryError;

/// Top-level error for the [`crate::manager::Manager`] actor.
#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Http(#[from] HttpServerError),
    #[error("actor messaging error: {0}")]
    Actor(String),
}
```

(If `src/error.rs` already has `use thiserror::Error;` at the top, drop the inline `thiserror::Error` qualifier on the derive and use `Error` instead. Check the file before editing.)

- [ ] **Step 3: Verify build**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/error.rs
git commit -m "feat(error): ManagerError wrapping Llm/Memory/Http"
```

---

## Task 2: Manager skeleton + new() constructor

**Files:**
- Create: `src/manager.rs`
- Modify: `src/lib.rs`

This task creates the struct and the `new()` function that spawns all three children. The `Actor` impl and supervision handlers come in subsequent tasks.

- [ ] **Step 1: Create `src/manager.rs`**

```rust
//! Top-level supervisor actor. Owns and supervises [`LlmService`],
//! [`MemoryManager`], and [`HttpServer`].

use acktor::{Actor, Address};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

use crate::config::Config;
use crate::error::ManagerError;
use crate::http::HttpServer;
use crate::llm::{LlmService, provider::build_provider};
use crate::memory::manager::MemoryManager;

pub struct Manager {
    llm: Address<LlmService>,
    memory: Address<MemoryManager>,
    http: Address<HttpServer>,
    llm_handle: Option<JoinHandle<()>>,
    memory_handle: Option<JoinHandle<()>>,
    http_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    shutting_down: bool,
}

impl Manager {
    /// Build the Manager: spawns LLM, then MemoryManager, then HttpServer.
    /// On failure any already-spawned children are dropped (their JoinHandles
    /// are dropped with them; acktor stops them when their mailbox closes).
    pub fn new(config: Config, shutdown_tx: oneshot::Sender<()>) -> Result<Self, ManagerError> {
        let provider = build_provider(&config.llm)?;
        let llm = LlmService::new(config.llm, provider);
        let (llm_addr, llm_handle) = llm
            .start("llm-service")
            .map_err(|e| ManagerError::Actor(format!("LlmService start: {e}")))?;
        info!("LlmService started");

        let memory = MemoryManager::new(config.memory, llm_addr.clone())?;
        let (memory_addr, memory_handle) = memory
            .start("memory-manager")
            .map_err(|e| ManagerError::Actor(format!("MemoryManager start: {e}")))?;
        info!("MemoryManager started");

        let http = HttpServer::new(config.server, memory_addr.clone());
        let (http_addr, http_handle) = http
            .start("http-server")
            .map_err(|e| ManagerError::Actor(format!("HttpServer start: {e}")))?;
        info!("HttpServer started");

        Ok(Self {
            llm: llm_addr,
            memory: memory_addr,
            http: http_addr,
            llm_handle: Some(llm_handle),
            memory_handle: Some(memory_handle),
            http_handle: Some(http_handle),
            shutdown_tx: Some(shutdown_tx),
            shutting_down: false,
        })
    }

    /// Test seam: returns the HttpServer address so tests can simulate child
    /// death by signal-terminating it directly. Not used by production code.
    pub fn http_addr(&self) -> &Address<HttpServer> {
        &self.http
    }
}
```

- [ ] **Step 2: Register module in `src/lib.rs`**

Add `pub mod manager;` to `src/lib.rs`, alphabetically:

```rust
pub mod config;
pub mod error;
pub mod http;
pub mod llm;
pub mod manager;
pub mod memory;
```

- [ ] **Step 3: Verify build**

Run: `cargo build`
Expected: PASS. The struct is not yet used externally so unused-field warnings on `llm`, `llm_handle`, `memory_handle`, `http_handle`, `shutdown_tx`, `shutting_down` are expected — they are wired up in later tasks. Use `#[allow(dead_code)]` ONLY if a real warning blocks the build; otherwise leave the fields alone.

- [ ] **Step 4: Commit**

```bash
git add src/manager.rs src/lib.rs
git commit -m "feat(manager): Manager struct and constructor spawning children"
```

---

## Task 3: Actor impl with post_start (supervisor subscriptions) and post_stop (drain children)

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Add imports and Actor impl**

In `src/manager.rs`, expand the imports block:

```rust
use acktor::{Actor, Address, Context, ErrorReport, Recipient, Signal};
use acktor::supervisor::{SupervisionEvent, Supervisor};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, trace, warn};
```

Append this `Actor` impl after the `impl Manager { ... }` block:

```rust
impl Actor for Manager {
    type Context = Context<Self>;
    type Error = ManagerError;

    async fn post_start(&mut self, ctx: &mut Self::Context) -> Result<(), ManagerError> {
        trace!("Manager post_start: subscribing supervisor events");

        let llm_recipient: Recipient<SupervisionEvent<LlmService>> = ctx.address().into();
        self.llm
            .send(Supervisor::Set(llm_recipient))
            .await
            .map_err(|e| ManagerError::Actor(format!("Set LLM supervisor: {e}")))?;

        let memory_recipient: Recipient<SupervisionEvent<MemoryManager>> = ctx.address().into();
        self.memory
            .send(Supervisor::Set(memory_recipient))
            .await
            .map_err(|e| ManagerError::Actor(format!("Set MemoryManager supervisor: {e}")))?;

        let http_recipient: Recipient<SupervisionEvent<HttpServer>> = ctx.address().into();
        self.http
            .send(Supervisor::Set(http_recipient))
            .await
            .map_err(|e| ManagerError::Actor(format!("Set HttpServer supervisor: {e}")))?;

        info!("Manager is supervising LlmService, MemoryManager, HttpServer");
        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<(), ManagerError> {
        // Drain in reverse startup order: HTTP first (stop accepting work),
        // then MemoryManager (flush in-flight ops), then LLM.
        if let Some(handle) = self.http_handle.take() {
            if let Err(e) = self.http.do_send(Signal::Terminate).await {
                warn!("Could not signal HttpServer: {}", e.report());
                handle.abort();
            }
            if let Err(e) = handle.await {
                warn!("HttpServer join error: {e}");
            }
        }
        if let Some(handle) = self.memory_handle.take() {
            if let Err(e) = self.memory.do_send(Signal::Terminate).await {
                warn!("Could not signal MemoryManager: {}", e.report());
                handle.abort();
            }
            if let Err(e) = handle.await {
                warn!("MemoryManager join error: {e}");
            }
        }
        if let Some(handle) = self.llm_handle.take() {
            if let Err(e) = self.llm.do_send(Signal::Terminate).await {
                warn!("Could not signal LlmService: {}", e.report());
                handle.abort();
            }
            if let Err(e) = handle.await {
                warn!("LlmService join error: {e}");
            }
        }
        info!("Manager is stopped");
        Ok(())
    }
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: PASS. Some warnings about unused `shutdown_tx` / `shutting_down` remain — handlers come in Task 4.

- [ ] **Step 3: Format**

Run: `cargo fmt`

- [ ] **Step 4: Commit**

```bash
git add src/manager.rs
git commit -m "feat(manager): Actor impl with supervisor subscriptions and post_stop drain"
```

---

## Task 4: SupervisionEvent handlers and initiate_shutdown

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Add `tracing::error` to imports**

Update the tracing import line in `src/manager.rs`:

```rust
use tracing::{debug, error, info, trace, warn};
```

- [ ] **Step 2: Add `initiate_shutdown` and three Handler impls**

Append at the end of `src/manager.rs`:

```rust
use acktor::Handler;
use std::future::Future;

impl Manager {
    /// Begins teardown: fire the shutdown oneshot so `main` exits, then
    /// signal-terminate the surviving children. Idempotent — second call
    /// observes `shutting_down` and returns.
    fn initiate_shutdown(&mut self, child: &str) {
        if self.shutting_down {
            debug!("initiate_shutdown ignored (already shutting down), trigger={child}");
            return;
        }
        self.shutting_down = true;
        error!("Manager initiating shutdown after {child} death");

        if let Some(tx) = self.shutdown_tx.take() {
            // Receiver may already be dropped if main exited via ctrl-c; ignore.
            let _ = tx.send(());
        }
    }
}

impl Handler<SupervisionEvent<LlmService>> for Manager {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<LlmService>,
        _ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        trace!("Manager: SupervisionEvent<LlmService>");
        match msg {
            SupervisionEvent::Warn(_, e) => warn!("LlmService warning: {e}"),
            SupervisionEvent::State(_, s) => debug!("LlmService state: {s:?}"),
            SupervisionEvent::Terminated(_, e) => {
                error!("LlmService terminated: {e:?}");
                self.initiate_shutdown("LlmService");
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("LlmService panicked: {info}");
                self.initiate_shutdown("LlmService");
            }
        }
        std::future::ready(())
    }
}

impl Handler<SupervisionEvent<MemoryManager>> for Manager {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<MemoryManager>,
        _ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        trace!("Manager: SupervisionEvent<MemoryManager>");
        match msg {
            SupervisionEvent::Warn(_, e) => warn!("MemoryManager warning: {e}"),
            SupervisionEvent::State(_, s) => debug!("MemoryManager state: {s:?}"),
            SupervisionEvent::Terminated(_, e) => {
                error!("MemoryManager terminated: {e:?}");
                self.initiate_shutdown("MemoryManager");
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("MemoryManager panicked: {info}");
                self.initiate_shutdown("MemoryManager");
            }
        }
        std::future::ready(())
    }
}

impl Handler<SupervisionEvent<HttpServer>> for Manager {
    type Result = ();

    fn handle(
        &mut self,
        msg: SupervisionEvent<HttpServer>,
        _ctx: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        trace!("Manager: SupervisionEvent<HttpServer>");
        match msg {
            SupervisionEvent::Warn(_, e) => warn!("HttpServer warning: {e}"),
            SupervisionEvent::State(_, s) => debug!("HttpServer state: {s:?}"),
            SupervisionEvent::Terminated(_, e) => {
                error!("HttpServer terminated: {e:?}");
                self.initiate_shutdown("HttpServer");
            }
            SupervisionEvent::Panicked(_, info) => {
                error!("HttpServer panicked: {info}");
                self.initiate_shutdown("HttpServer");
            }
        }
        std::future::ready(())
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build`
Expected: PASS, no warnings about `shutting_down` or `shutdown_tx` anymore.

- [ ] **Step 4: Format**

Run: `cargo fmt`

- [ ] **Step 5: Commit**

```bash
git add src/manager.rs
git commit -m "feat(manager): SupervisionEvent handlers and shutdown initiation"
```

---

## Task 5: Refactor main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace `src/main.rs` with:**

```rust
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
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: PASS. The `clawchorus::{http::HttpServer, llm::..., memory::manager::MemoryManager}` imports are gone; only `Manager` is needed.

- [ ] **Step 3: Format**

Run: `cargo fmt`

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): wire Manager with ctrl-c and shutdown_rx select"
```

---

## Task 6: Integration tests

**Files:**
- Create: `tests/manager_integration.rs`
- Modify: `src/manager.rs` (add `new_with_provider` test seam)

`Manager::new` uses `build_provider(&config.llm)` which reads a string and constructs the real provider. Tests need to inject `MockProvider` instead. Add a `Manager::new_with_provider` constructor that bypasses `build_provider` and accepts an explicit `Arc<dyn Provider>`. It is unconditionally `pub` — a documented test seam, same philosophy as the previously-exposed `MockProvider`. No feature flags.

- [ ] **Step 1: Add the `new_with_provider` constructor**

In `src/manager.rs`, immediately after `Manager::new`:

```rust
    /// Test seam: build a Manager with a caller-supplied LLM provider,
    /// bypassing `build_provider`. Lets tests inject `MockProvider`.
    pub fn new_with_provider(
        config: Config,
        provider: std::sync::Arc<dyn crate::llm::provider::Provider>,
        shutdown_tx: oneshot::Sender<()>,
    ) -> Result<Self, ManagerError> {
        let llm = LlmService::new(config.llm, provider);
        let (llm_addr, llm_handle) = llm
            .start("llm-service")
            .map_err(|e| ManagerError::Actor(format!("LlmService start: {e}")))?;

        let memory = MemoryManager::new(config.memory, llm_addr.clone())?;
        let (memory_addr, memory_handle) = memory
            .start("memory-manager")
            .map_err(|e| ManagerError::Actor(format!("MemoryManager start: {e}")))?;

        let http = HttpServer::new(config.server, memory_addr.clone());
        let (http_addr, http_handle) = http
            .start("http-server")
            .map_err(|e| ManagerError::Actor(format!("HttpServer start: {e}")))?;

        Ok(Self {
            llm: llm_addr,
            memory: memory_addr,
            http: http_addr,
            llm_handle: Some(llm_handle),
            memory_handle: Some(memory_handle),
            http_handle: Some(http_handle),
            shutdown_tx: Some(shutdown_tx),
            shutting_down: false,
        })
    }
```

- [ ] **Step 2: Verify build and trait path**

Run: `cargo build`. If `crate::llm::provider::Provider` isn't the exact path, grep `src/llm/provider.rs` for `pub trait Provider` and adjust the parameter type to match (likely `crate::llm::Provider` or `crate::llm::provider::Provider`). `MockProvider` lives at `crate::llm::provider::mock::MockProvider`.

- [ ] **Step 3: Create `tests/manager_integration.rs`**

```rust
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
    let shutdown =
        tokio::time::timeout(Duration::from_secs(2), shutdown_rx).await;
    assert!(
        shutdown.is_ok() && shutdown.unwrap().is_ok(),
        "shutdown_rx did not fire after HttpServer death"
    );

    // Manager itself should also terminate cleanly within a generous window.
    let _ = tokio::time::timeout(Duration::from_secs(5), manager_handle).await;
}
```

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 5: Run integration tests**

Run: `cargo test --test manager_integration`
Expected: 2/2 PASS.

If a test hangs or fails, debug:
- For `manager_starts_and_stops_cleanly`: confirm `post_stop` drains all three child handles. A hang here usually means `do_send(Signal::Terminate)` succeeded but the child's `JoinHandle` never resolves — check that the child actor's `post_stop` returns.
- For `manager_initiates_shutdown_when_child_dies`: confirm Manager actually receives the `SupervisionEvent::Terminated`. If it doesn't, the supervisor subscription in `post_start` is failing silently. Add `trace!` logs in `post_start` and run with `RUST_LOG=trace`.

- [ ] **Step 6: Verify nothing else broke**

Run: `cargo test`
Expected: All 77+2 tests PASS.

- [ ] **Step 7: Format**

Run: `cargo fmt`

- [ ] **Step 8: Commit**

```bash
git add src/manager.rs tests/manager_integration.rs
git commit -m "test(manager): integration tests for clean stop and child-death shutdown"
```

---

## Task 7: Final fmt + tests + clippy

**Files:** All previously touched.

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no diff.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test`
Expected: ALL tests PASS (existing 77 + 2 new integration tests = 79).

- [ ] **Step 3: Clippy**

Run: `cargo clippy --all-targets -- -W clippy::all`
Expected: Three pre-existing warnings (MockProvider Default, transmute annotation in `index.rs`, collapsible_if in `synthesizer.rs`) plus zero new warnings in `src/manager.rs`. If clippy reports a new warning in our new code, fix it.

- [ ] **Step 4: Commit any fmt/lint fixes**

```bash
git add -u
git diff --cached --quiet || git commit -m "style: cargo fmt"
```

- [ ] **Step 5: Sanity-run the binary**

In one shell (PowerShell): `cargo run`
In another: `Invoke-RestMethod http://127.0.0.1:8080/health`
Expected: `{"status":"ok"}`. Then `Ctrl-C` in the first shell. Look for log lines:
```
ctrl-c received, shutting down
HttpServer is stopped
MemoryManager is stopped
Manager is stopped
Manager stopped cleanly
```
(Order in the middle three lines may vary slightly.)

Do NOT commit any artifacts from this smoke test.
