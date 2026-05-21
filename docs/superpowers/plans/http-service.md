# HTTP Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose ClawChorus's `MemoryManager` over a small JSON HTTP API consumed by OpenClaw agents.

**Architecture:** A single `HttpServer` actor owns an `axum::serve` task and is supervised alongside `MemoryManager` and `LlmService`. Free-function handlers in an Axum router deserialize JSON, send one message to `MemoryManager`, and serialize the reply. Identity (`username`, `agent_id`) is carried in request bodies.

**Tech Stack:** Rust 2024, Axum 0.8, Tokio, acktor 1.1, serde / serde_json, tower-http, thiserror, tracing. Dev: tempfile, `tower::ServiceExt::oneshot` for in-process router tests (no real network bind).

**Spec:** `docs/superpowers/specs/http-service-design.md`

---

## File Structure

```
src/
  http.rs              // HttpServer actor + spawn + re-exports
  http/
    error.rs           // HttpError enum + IntoResponse impl
    dto.rs             // request/response JSON structs
    handlers.rs        // free async handlers
    router.rs          // build_router(addr) -> axum::Router
  lib.rs               // add `pub mod http;`
  main.rs              // wire HttpServer after MemoryManager
Cargo.toml             // add tower, tower-http; bump dev-deps with `tower` if missing
tests/
  http_integration.rs  // end-to-end router tests against a stub MemoryManager
```

---

## Task 1: Add HTTP dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add deps**

Add to the `[dependencies]` block in `Cargo.toml` (keep alphabetical order):

```toml
tower-http = { version = "0.6", features = ["trace"] }
```

Add to the `[dev-dependencies]` block (used by integration tests for `ServiceExt::oneshot`):

```toml
tower = "0.5"
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: PASS (no compile errors; new crates downloaded).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add tower and tower-http for HTTP service"
```

---

## Task 2: Scaffold http module

**Files:**
- Create: `src/http.rs`
- Create: `src/http/error.rs`
- Create: `src/http/dto.rs`
- Create: `src/http/handlers.rs`
- Create: `src/http/router.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create empty submodule files**

`src/http/error.rs`:

```rust
//! HTTP error type and response mapping.
```

`src/http/dto.rs`:

```rust
//! Request and response JSON structs for the HTTP API.
```

`src/http/handlers.rs`:

```rust
//! Axum handler functions for the HTTP API.
```

`src/http/router.rs`:

```rust
//! Axum router construction for the HTTP API.
```

- [ ] **Step 2: Create `src/http.rs`**

```rust
//! HTTP service: Axum-based JSON API over the Memory Manager actor.

pub mod dto;
pub mod error;
pub mod handlers;
pub mod router;
```

- [ ] **Step 3: Register the module in `src/lib.rs`**

Modify `src/lib.rs` to add `pub mod http;` (keep alphabetical):

```rust
pub mod config;
pub mod error;
pub mod http;
pub mod llm;
pub mod memory;
```

- [ ] **Step 4: Verify build**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/http.rs src/http/ src/lib.rs
git commit -m "feat(http): scaffold http module"
```

---

## Task 3: DTOs and serde round-trip tests

**Files:**
- Modify: `src/http/dto.rs`

- [ ] **Step 1: Write the failing tests**

Replace `src/http/dto.rs` with:

```rust
//! Request and response JSON structs for the HTTP API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::messages::SearchResult;

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub query: String,
}

#[derive(Debug, Serialize)]
pub struct ReadResponse {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_request_deserializes() {
        let json = r#"{
            "username": "alice",
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "filename": "2026-05-13.md",
            "content": "hello"
        }"#;
        let req: WriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "alice");
        assert_eq!(
            req.agent_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(req.filename, "2026-05-13.md");
        assert_eq!(req.content, "hello");
    }

    #[test]
    fn search_request_deserializes() {
        let json = r#"{
            "username": "alice",
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "query": "rust"
        }"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "rust");
    }

    #[test]
    fn read_response_serializes() {
        let resp = ReadResponse { content: "hi".to_string() };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"content":"hi"}"#);
    }

    #[test]
    fn health_response_serializes() {
        let resp = HealthResponse { status: "ok" };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"status":"ok"}"#);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib http::dto`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add src/http/dto.rs
git commit -m "feat(http): request/response DTOs"
```

---

## Task 4: HttpError with IntoResponse

**Files:**
- Modify: `src/http/error.rs`

- [ ] **Step 1: Write the failing tests + implementation**

Replace `src/http/error.rs` with:

```rust
//! HTTP error type and response mapping.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

use crate::memory::error::MemoryError;

/// Error type returned by HTTP handlers.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("not found")]
    NotFound,
    #[error("memory error: {0}")]
    Memory(#[from] MemoryError),
    #[error("service unavailable: {0}")]
    Unavailable(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            HttpError::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            HttpError::Memory(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                Some(e.to_string()),
            ),
            HttpError::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable", None),
        };
        (
            status,
            Json(ErrorBody {
                error: code,
                message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_string(resp: Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let (status, body) = body_string(HttpError::NotFound.into_response()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, r#"{"error":"not_found"}"#);
    }

    #[tokio::test]
    async fn unavailable_maps_to_503() {
        let (status, body) =
            body_string(HttpError::Unavailable("dead".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, r#"{"error":"unavailable"}"#);
    }

    #[tokio::test]
    async fn memory_error_maps_to_500_with_message() {
        let err = HttpError::Memory(MemoryError::Actor("boom".to_string()));
        let (status, body) = body_string(err.into_response()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body.contains(r#""error":"internal""#));
        assert!(body.contains("boom"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib http::error`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/http/error.rs
git commit -m "feat(http): HttpError with IntoResponse mapping"
```

---

## Task 5: Handlers

**Files:**
- Modify: `src/http/handlers.rs`

Note: Each handler does `addr.send(msg).await` (mailbox send) and then `.await` again because `MemoryManager` returns `FutureMessageResult` (see `src/memory/manager.rs`). Mailbox send errors map to `HttpError::Unavailable`. `MemoryError` maps via `?` (its `From` impl into `HttpError`).

- [ ] **Step 1: Implement handlers**

Replace `src/http/handlers.rs` with:

```rust
//! Axum handler functions for the HTTP API.

use acktor::Address;
use axum::Json;
use axum::extract::State;

use crate::http::dto::{
    DeleteRequest, HealthResponse, ReadRequest, ReadResponse, SearchRequest, SearchResponse,
    WriteRequest,
};
use crate::http::error::HttpError;
use crate::memory::manager::MemoryManager;
use crate::memory::messages::{FileOpDelete, FileOpRead, FileOpWrite, Search};

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn write(
    State(mm): State<Address<MemoryManager>>,
    Json(req): Json<WriteRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let msg = FileOpWrite {
        username: req.username,
        agent_id: req.agent_id,
        filename: req.filename,
        content: req.content,
    };
    mm.send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    Ok(Json(serde_json::json!({})))
}

pub async fn read(
    State(mm): State<Address<MemoryManager>>,
    Json(req): Json<ReadRequest>,
) -> Result<Json<ReadResponse>, HttpError> {
    let msg = FileOpRead {
        username: req.username,
        agent_id: req.agent_id,
        filename: req.filename,
    };
    let content = mm
        .send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    match content {
        Some(content) => Ok(Json(ReadResponse { content })),
        None => Err(HttpError::NotFound),
    }
}

pub async fn delete(
    State(mm): State<Address<MemoryManager>>,
    Json(req): Json<DeleteRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let msg = FileOpDelete {
        username: req.username,
        agent_id: req.agent_id,
        filename: req.filename,
    };
    mm.send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    Ok(Json(serde_json::json!({})))
}

pub async fn search(
    State(mm): State<Address<MemoryManager>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, HttpError> {
    let msg = Search {
        username: req.username,
        agent_id: req.agent_id,
        query: req.query,
    };
    let results = mm
        .send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    Ok(Json(SearchResponse { results }))
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build --lib`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/http/handlers.rs
git commit -m "feat(http): handlers for write/read/delete/search/health"
```

---

## Task 6: Router

**Files:**
- Modify: `src/http/router.rs`

- [ ] **Step 1: Implement router**

Replace `src/http/router.rs` with:

```rust
//! Axum router construction for the HTTP API.

use acktor::Address;
use axum::Router;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

use crate::http::handlers::{delete, health, read, search, write};
use crate::memory::manager::MemoryManager;

pub fn build_router(mm: Address<MemoryManager>) -> Router {
    let v1 = Router::new()
        .route("/health", get(health))
        .route("/memories/write", post(write))
        .route("/memories/read", post(read))
        .route("/memories/delete", post(delete))
        .route("/search", post(search));
    Router::new()
        .nest("/v1", v1)
        .layer(TraceLayer::new_for_http())
        .with_state(mm)
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build --lib`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/http/router.rs
git commit -m "feat(http): build_router with TraceLayer"
```

---

## Task 7: Integration test — happy path

**Files:**
- Create: `tests/http_integration.rs`

This uses the real `MemoryManager` with `MockProvider` and `:memory:` SQLite — same pattern as `src/memory/manager.rs` tests. Router is driven via `tower::ServiceExt::oneshot`, so no port is bound.

- [ ] **Step 1: Write the test**

Create `tests/http_integration.rs`:

```rust
use acktor::{Actor, Address};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use clawchorus::http::router::build_router;
use clawchorus::llm::LlmService;
use clawchorus::llm::provider::mock::MockProvider;
use clawchorus::memory::config::MemoryConfig;
use clawchorus::memory::manager::MemoryManager;
use std::sync::Arc;
use tower::ServiceExt;

fn test_config(dir: &std::path::Path) -> MemoryConfig {
    MemoryConfig {
        memory_dir: dir.to_string_lossy().to_string(),
        db_path: ":memory:".to_string(),
        ..MemoryConfig::default()
    }
}

async fn spawn_memory_manager(dir: &std::path::Path) -> Address<MemoryManager> {
    let provider = Arc::new(MockProvider::new());
    let llm = LlmService::new(Default::default(), provider);
    let (llm_addr, _llm_handle) = llm.start("llm-test").unwrap();
    let mm = MemoryManager::new(test_config(dir), llm_addr).unwrap();
    let (mm_addr, _mm_handle) = mm.start("memory-manager").unwrap();
    Box::leak(Box::new(_mm_handle));
    Box::leak(Box::new(_llm_handle));
    mm_addr
}

async fn body_string(resp: axum::response::Response) -> (StatusCode, String) {
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn health_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let app = build_router(mm);

    let req = Request::builder()
        .uri("/v1/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let (status, body) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"status":"ok"}"#);
}

#[tokio::test]
async fn write_then_read_then_delete_then_read_404() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let app = build_router(mm);

    let agent_id = "550e8400-e29b-41d4-a716-446655440000";

    // Write.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/write")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "filename":"test.md",
                "content":"hello"
            }}"#
        )))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "{}");

    // Read.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/read")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "filename":"test.md"
            }}"#
        )))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"content":"hello"}"#);

    // Delete.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/delete")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "filename":"test.md"
            }}"#
        )))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, _body) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);

    // Read after delete -> 404.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/read")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "filename":"test.md"
            }}"#
        )))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let (status, body) = body_string(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"not_found"}"#);
}

#[tokio::test]
async fn search_after_write_returns_results() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let app = build_router(mm);

    let agent_id = "550e8400-e29b-41d4-a716-446655440000";

    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/write")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "filename":"notes.md",
                "content":"Rust programming language is great"
            }}"#
        )))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/search")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "query":"programming"
            }}"#
        )))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let (status, body) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""results""#));
    assert!(body.contains("notes.md"));
}

#[tokio::test]
async fn bad_json_returns_400() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let app = build_router(mm);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/write")
        .header("content-type", "application/json")
        .body(Body::from("{not valid json}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --test http_integration`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add tests/http_integration.rs
git commit -m "test(http): integration tests for router + handlers"
```

---

## Task 8: HttpServer actor

**Files:**
- Modify: `src/http.rs`

The actor owns the `axum::serve` join handle and aborts it on stop. It has no message handlers — it exists for lifecycle integration with `acktor` supervision.

- [ ] **Step 1: Implement the actor**

Replace `src/http.rs` with:

```rust
//! HTTP service: Axum-based JSON API over the Memory Manager actor.

pub mod dto;
pub mod error;
pub mod handlers;
pub mod router;

use std::io;
use std::net::SocketAddr;

use acktor::{Actor, Address, Context};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{info, trace};

use crate::config::ServerConfig;
use crate::http::router::build_router;
use crate::memory::manager::MemoryManager;

#[derive(Debug, Error)]
pub enum HttpServerError {
    #[error("bind error on {addr}: {source}")]
    Bind { addr: String, source: io::Error },
    #[error("invalid bind address {addr}: {source}")]
    InvalidAddr {
        addr: String,
        source: std::net::AddrParseError,
    },
}

pub struct HttpServer {
    config: ServerConfig,
    memory_manager: Address<MemoryManager>,
    serve_handle: Option<JoinHandle<()>>,
}

impl HttpServer {
    pub fn new(config: ServerConfig, memory_manager: Address<MemoryManager>) -> Self {
        Self {
            config,
            memory_manager,
            serve_handle: None,
        }
    }
}

impl Actor for HttpServer {
    type Context = Context<Self>;
    type Error = HttpServerError;

    async fn post_start(&mut self, _ctx: &mut Self::Context) -> Result<(), HttpServerError> {
        trace!("HttpServer post_start");
        let addr_str = format!("{}:{}", self.config.host, self.config.port);
        let addr: SocketAddr = addr_str
            .parse()
            .map_err(|source| HttpServerError::InvalidAddr {
                addr: addr_str.clone(),
                source,
            })?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| HttpServerError::Bind {
                addr: addr_str.clone(),
                source,
            })?;
        let app = build_router(self.memory_manager.clone());
        info!(%addr, "HttpServer listening");
        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("axum serve error: {e}");
            }
        });
        self.serve_handle = Some(handle);
        Ok(())
    }

    async fn post_stop(&mut self, _ctx: &mut Self::Context) -> Result<(), HttpServerError> {
        if let Some(handle) = self.serve_handle.take() {
            handle.abort();
        }
        info!("HttpServer is stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmService;
    use crate::llm::provider::mock::MockProvider;
    use crate::memory::config::MemoryConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn http_server_starts_and_stops() {
        let provider = Arc::new(MockProvider::new());
        let llm = LlmService::new(Default::default(), provider);
        let (llm_addr, _llm_handle) = llm.start("llm-test").unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mm_cfg = MemoryConfig {
            memory_dir: dir.path().to_string_lossy().to_string(),
            db_path: ":memory:".to_string(),
            ..MemoryConfig::default()
        };
        let mm = MemoryManager::new(mm_cfg, llm_addr).unwrap();
        let (mm_addr, _mm_handle) = mm.start("memory-manager").unwrap();

        // Pick an ephemeral port by binding 0.
        let cfg = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
        };
        let server = HttpServer::new(cfg, mm_addr);
        let (server_addr, server_handle) = server.start("http-server").unwrap();

        // Stop the actor cleanly.
        server_addr.do_send(acktor::Signal::Terminate).await.unwrap();
        let _ = server_handle.await;
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib http`
Expected: PASS (all `http::*` unit tests including the actor start/stop test).

- [ ] **Step 3: Commit**

```bash
git add src/http.rs
git commit -m "feat(http): HttpServer actor owning axum::serve task"
```

---

## Task 9: Wire HttpServer into main

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Update main**

Replace `src/main.rs` with:

```rust
use acktor::Actor;
use anyhow::Result;
use tracing::info;

use clawchorus::{
    config,
    http::HttpServer,
    llm::{LlmService, provider::build_provider},
    memory::manager::MemoryManager,
};

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

    let provider = build_provider(&config.llm)?;
    let llm = LlmService::new(config.llm, provider);
    let (llm_addr, _llm_handle) = llm.start("llm-service")?;

    let mm = MemoryManager::new(config.memory, llm_addr)?;
    let (mm_addr, _mm_handle) = mm.start("memory-manager")?;
    info!("Memory Manager started");

    let http = HttpServer::new(config.server, mm_addr);
    let (_http_addr, _http_handle) = http.start("http-server")?;
    info!("HTTP server started");

    tokio::signal::ctrl_c().await?;
    info!("Shutting down");

    Ok(())
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 3: Smoke test**

Run (in background): `cargo run`
In another shell: `curl -s http://127.0.0.1:8080/v1/health`
Expected: `{"status":"ok"}`
Then stop the server with Ctrl-C.

(If running on Windows PowerShell, use `Invoke-RestMethod http://127.0.0.1:8080/v1/health` or `curl.exe`.)

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(http): wire HttpServer into main"
```

---

## Task 10: cargo fmt + final verification

**Files:**
- All `.rs` files touched in this plan.

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no errors. Per `CLAUDE.md`, run `cargo fmt` after modifying `.rs` files.

- [ ] **Step 2: Run the full suite**

Run: `cargo test`
Expected: PASS (all existing tests + new `http::*` unit tests + `tests/http_integration.rs`).

- [ ] **Step 3: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: PASS (no warnings).

- [ ] **Step 4: Commit any formatting changes**

```bash
git add -u
git diff --cached --quiet || git commit -m "style: cargo fmt"
```
