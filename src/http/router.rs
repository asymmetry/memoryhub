use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use super::HttpServerState;
use super::error::HttpError;
use crate::memory::{
    MemoryType,
    messages::{FileOpDelete, FileOpRead, FileOpWrite, Search, SearchResult},
};

pub fn build_router(state: HttpServerState) -> Router {
    let v1 = Router::new()
        .route("/health", get(health))
        .route("/memories/write", post(write))
        .route("/memories/read", post(read))
        .route("/memories/delete", post(delete))
        .route("/search", post(search));
    Router::new()
        .nest("/v1", v1)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub memory_type: MemoryType,
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub memory_type: MemoryType,
    pub filename: String,
}

#[derive(Debug, Serialize)]
pub struct ReadResponse {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub memory_type: MemoryType,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub username: String,
    pub agent_id: Uuid,
    pub query: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn write(
    State(state): State<HttpServerState>,
    Json(req): Json<WriteRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let msg = FileOpWrite {
        username: req.username,
        agent_id: req.agent_id,
        memory_type: req.memory_type,
        filename: req.filename,
        content: req.content,
    };
    state
        .memory_manager
        .send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    Ok(Json(serde_json::json!({})))
}

pub async fn read(
    State(state): State<HttpServerState>,
    Json(req): Json<ReadRequest>,
) -> Result<Json<ReadResponse>, HttpError> {
    let msg = FileOpRead {
        username: req.username,
        agent_id: req.agent_id,
        memory_type: req.memory_type,
        filename: req.filename,
    };
    let content = state
        .memory_manager
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
    State(state): State<HttpServerState>,
    Json(req): Json<DeleteRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let msg = FileOpDelete {
        username: req.username,
        agent_id: req.agent_id,
        memory_type: req.memory_type,
        filename: req.filename,
    };
    state
        .memory_manager
        .send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    Ok(Json(serde_json::json!({})))
}

pub async fn search(
    State(state): State<HttpServerState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, HttpError> {
    let msg = Search {
        username: req.username,
        agent_id: req.agent_id,
        query: req.query,
    };
    let results = state
        .memory_manager
        .send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    Ok(Json(SearchResponse { results }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_request_deserializes_snake_case_memory_type() {
        let json = r#"{
            "username": "alice",
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "memory_type": "daily_note",
            "filename": "2026-05-13.md",
            "content": "hello"
        }"#;
        let req: WriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "alice");
        assert_eq!(req.memory_type, MemoryType::DailyNote);
        assert_eq!(req.filename, "2026-05-13.md");
        assert_eq!(req.content, "hello");
    }

    #[test]
    fn write_request_rejects_unknown_memory_type() {
        let json = r#"{
            "username": "alice",
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "memory_type": "garbage",
            "filename": "x.md",
            "content": ""
        }"#;
        assert!(serde_json::from_str::<WriteRequest>(json).is_err());
    }

    #[test]
    fn read_request_long_term_memory_type() {
        let json = r#"{
            "username": "bob",
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "memory_type": "long_term",
            "filename": "MEMORY.md"
        }"#;
        let req: ReadRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.memory_type, MemoryType::LongTerm);
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
        let resp = ReadResponse {
            content: "hi".to_string(),
        };
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
