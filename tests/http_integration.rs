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
        .uri("/health")
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
        .uri("/memories/write")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "memory_type":"daily_note",
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
        .uri("/memories/read")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "memory_type":"daily_note",
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
        .uri("/memories/delete")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "memory_type":"daily_note",
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
        .uri("/memories/read")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "memory_type":"daily_note",
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
        .uri("/memories/write")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{
                "username":"alice",
                "agent_id":"{agent_id}",
                "memory_type":"daily_note",
                "filename":"notes.md",
                "content":"Rust programming language is great"
            }}"#
        )))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("POST")
        .uri("/search")
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
        .uri("/memories/write")
        .header("content-type", "application/json")
        .body(Body::from("{not valid json}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
