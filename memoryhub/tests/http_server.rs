use std::sync::Arc;

use acktor::{Actor, Address, Signal};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use memoryhub::{
    config::ServerConfig,
    http::{AuthConfig, AuthStore, HttpServer, HttpServerState, build_router},
    llm::{LlmConfig, LlmService},
    memory::{MemoryConfig, MemoryManager},
};

fn test_config(dir: &std::path::Path) -> MemoryConfig {
    MemoryConfig {
        memory_dir: dir.to_string_lossy().to_string(),
        db_path: ":memory:".to_string(),
        ..MemoryConfig::default()
    }
}

async fn spawn_memory_manager(dir: &std::path::Path) -> Address<MemoryManager> {
    let llm_cfg = LlmConfig {
        provider: "mock".into(),
        embedding_provider: "mock".into(),
        prompts_dir: dir.join("prompts"),
        ..Default::default()
    };
    let (llm_addr, _llm_handle) = LlmService::new(llm_cfg).start("llm-test").unwrap();
    let mm = MemoryManager::new(test_config(dir), llm_addr);
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

/// Builds handler state with an in-memory auth store, a `user`-role account `alice`, and a
/// freshly minted token for her. Returns the state and the token secret.
async fn state_with_user(mm: Address<MemoryManager>) -> (HttpServerState, String) {
    let store = Arc::new(AuthStore::open_in_memory(Some("mh_root".into())).unwrap());
    store.create_user("alice", "user").await.unwrap();
    let token = store
        .create_token("alice", None, None)
        .await
        .unwrap()
        .secret;
    (HttpServerState::new(mm, store), token)
}

#[tokio::test]
async fn http_server_starts_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;

    // Pick an ephemeral port by binding 0.
    let cfg = ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
    };
    let auth_cfg = AuthConfig {
        db_path: ":memory:".into(),
        admin_token: Some("mh_root".into()),
    };
    let server = HttpServer::new(cfg, auth_cfg, mm);
    let (server_addr, server_handle) = server.start("http-server").unwrap();

    // Stop the actor cleanly.
    server_addr.do_send(Signal::Terminate).await.unwrap();
    let _ = server_handle.await;
}

#[tokio::test]
async fn health_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let (state, _token) = state_with_user(mm).await;
    let app = build_router(state);

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
    let (state, token) = state_with_user(mm).await;
    let app = build_router(state);

    let agent_id = "550e8400-e29b-41d4-a716-446655440000";
    let bearer = format!("Bearer {}", token);

    // Write.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/write")
        .header("content-type", "application/json")
        .header("authorization", &bearer)
        .body(Body::from(format!(
            r#"{{
                "agent_id":"{}",
                "filename":"test.md",
                "content":"hello"
            }}"#,
            agent_id
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
        .header("authorization", &bearer)
        .body(Body::from(format!(
            r#"{{
                "agent_id":"{}",
                "filename":"test.md"
            }}"#,
            agent_id
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
        .header("authorization", &bearer)
        .body(Body::from(format!(
            r#"{{
                "agent_id":"{}",
                "filename":"test.md"
            }}"#,
            agent_id
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
        .header("authorization", &bearer)
        .body(Body::from(format!(
            r#"{{
                "agent_id":"{}",
                "filename":"test.md"
            }}"#,
            agent_id
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
    let (state, token) = state_with_user(mm).await;
    let app = build_router(state);

    let agent_id = "550e8400-e29b-41d4-a716-446655440000";
    let bearer = format!("Bearer {}", token);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/write")
        .header("content-type", "application/json")
        .header("authorization", &bearer)
        .body(Body::from(format!(
            r#"{{
                "agent_id":"{}",
                "filename":"notes.md",
                "content":"Rust programming language is great"
            }}"#,
            agent_id
        )))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/search")
        .header("content-type", "application/json")
        .header("authorization", &bearer)
        .body(Body::from(format!(
            r#"{{
                "agent_id":"{}",
                "query":"programming"
            }}"#,
            agent_id
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
    let (state, token) = state_with_user(mm).await;
    let app = build_router(state);

    // Authenticated so the request passes the auth middleware and reaches JSON parsing.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/write")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from("{not valid json}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn missing_token_is_401() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let (state, _token) = state_with_user(mm).await;
    let app = build_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories/read")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"agent_id":"550e8400-e29b-41d4-a716-446655440000","filename":"x.md"}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_can_create_user_and_mint_token() {
    let dir = tempfile::tempdir().unwrap();
    let mm = spawn_memory_manager(dir.path()).await;
    let store = Arc::new(AuthStore::open_in_memory(Some("mh_root".into())).unwrap());
    let app = build_router(HttpServerState::new(mm, store));

    // Create a user with the root token.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/admin/users")
        .header("content-type", "application/json")
        .header("authorization", "Bearer mh_root")
        .body(Body::from(r#"{"username":"bob","role":"user"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Mint a token for that user.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/admin/users/bob/tokens")
        .header("content-type", "application/json")
        .header("authorization", "Bearer mh_root")
        .body(Body::from(r#"{"name":"laptop"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let (status, body) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""token":"mh_"#));
}
