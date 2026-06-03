use axum::{
    Json, Router,
    extract::State,
    middleware::from_fn_with_state,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use super::error::HttpError;
use super::middleware::{AuthUser, auth_middleware};
use super::{HttpServerState, admin};
use crate::memory::message::{
    FileOpDelete, FileOpRead, FileOpWrite, GetSummary, Search, SearchResult, SearchScope,
    SummaryScope,
};

pub fn build_router(state: HttpServerState) -> Router {
    // Protected routes: everything under /v1 except /health. The middleware runs via
    // `route_layer`, which applies only to routes defined on this sub-router.
    let protected = Router::new()
        .route("/memories/write", post(write_memory))
        .route("/memories/read", post(read_memory))
        .route("/memories/delete", post(delete_memory))
        .route("/memories/search", post(search_memory))
        .route("/memories/summary", post(summary_memory))
        .route("/me", get(admin::me))
        .route(
            "/admin/users",
            post(admin::create_user).get(admin::list_users),
        )
        .route("/admin/users/{username}", delete(admin::delete_user))
        .route(
            "/admin/users/{username}/tokens",
            post(admin::create_token).get(admin::list_tokens),
        )
        .route("/admin/tokens/{id}", delete(admin::revoke_token))
        .route_layer(from_fn_with_state(state.clone(), auth_middleware));

    let v1 = Router::new().route("/health", get(health)).merge(protected);

    Router::new()
        .nest("/v1", v1)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub agent_id: Uuid,
    #[serde(default)]
    pub project: Option<String>,
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadRequest {
    pub agent_id: Uuid,
    #[serde(default)]
    pub project: Option<String>,
    pub filename: String,
}

#[derive(Debug, Serialize)]
pub struct ReadResponse {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub agent_id: Uuid,
    #[serde(default)]
    pub project: Option<String>,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    pub query: String,
    #[serde(default)]
    pub scope: SearchScope,
    #[serde(default)]
    pub raw_only: bool,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
pub struct SummaryRequest {
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    pub scope: SummaryScope,
}

#[derive(Debug, Serialize)]
pub struct SummaryResponse {
    pub content: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn write_memory(
    State(state): State<HttpServerState>,
    user: AuthUser,
    Json(req): Json<WriteRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let msg = FileOpWrite {
        username: user.username,
        agent_id: req.agent_id,
        project: req.project,
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

pub async fn read_memory(
    State(state): State<HttpServerState>,
    user: AuthUser,
    Json(req): Json<ReadRequest>,
) -> Result<Json<ReadResponse>, HttpError> {
    let msg = FileOpRead {
        username: user.username,
        agent_id: req.agent_id,
        project: req.project,
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

pub async fn delete_memory(
    State(state): State<HttpServerState>,
    user: AuthUser,
    Json(req): Json<DeleteRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let msg = FileOpDelete {
        username: user.username,
        agent_id: req.agent_id,
        project: req.project,
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

pub async fn search_memory(
    State(state): State<HttpServerState>,
    user: AuthUser,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, HttpError> {
    let msg = Search {
        username: user.username,
        agent_id: req.agent_id.unwrap_or_else(Uuid::nil),
        scope: req.scope,
        raw_only: req.raw_only,
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

pub async fn summary_memory(
    State(state): State<HttpServerState>,
    user: AuthUser,
    Json(req): Json<SummaryRequest>,
) -> Result<Json<SummaryResponse>, HttpError> {
    let msg = GetSummary {
        username: user.username,
        agent_id: req.agent_id,
        scope: req.scope,
    };
    let summary = state
        .memory_manager
        .send(msg)
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))?
        .await
        .map_err(|e| HttpError::Unavailable(e.to_string()))??;
    match summary {
        Some(s) => Ok(Json(SummaryResponse {
            content: s.content,
            path: s.path,
        })),
        None => Err(HttpError::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_request_deserializes() {
        let json = r#"{
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "filename": "2026-05-13.md",
            "content": "hello"
        }"#;
        let req: WriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.filename, "2026-05-13.md");
        assert_eq!(req.content, "hello");
    }

    #[test]
    fn write_request_without_project_defaults_none() {
        let json = r#"{"agent_id":"550e8400-e29b-41d4-a716-446655440000","filename":"x.md","content":"hi"}"#;
        let req: WriteRequest = serde_json::from_str(json).unwrap();
        assert!(req.project.is_none());
    }

    #[test]
    fn write_request_with_project() {
        let json = r#"{"agent_id":"550e8400-e29b-41d4-a716-446655440000","project":"p","filename":"x.md","content":"hi"}"#;
        let req: WriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.project.as_deref(), Some("p"));
    }

    #[test]
    fn read_request_deserializes() {
        let json = r#"{
            "agent_id": "550e8400-e29b-41d4-a716-446655440000",
            "filename": "notes/MEMORY.md"
        }"#;
        let req: ReadRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.filename, "notes/MEMORY.md");
    }

    #[test]
    fn search_request_defaults_scope_all() {
        let json = r#"{"query":"rust"}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "rust");
        assert_eq!(req.scope, SearchScope::All);
        assert!(!req.raw_only);
        assert!(req.agent_id.is_none());
    }

    #[test]
    fn search_request_parses_scope_and_raw_only() {
        let json = r#"{"agent_id":"550e8400-e29b-41d4-a716-446655440000","query":"x","scope":"agent","raw_only":true}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.scope, SearchScope::Agent);
        assert!(req.raw_only);
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

    #[test]
    fn summary_request_deserializes_with_scope() {
        let json = r#"{"agent_id":"550e8400-e29b-41d4-a716-446655440000","scope":"agent"}"#;
        let req: SummaryRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req.scope, SummaryScope::Agent));
        assert!(req.agent_id.is_some());
    }

    #[test]
    fn summary_request_allows_absent_agent_id() {
        let req: SummaryRequest = serde_json::from_str(r#"{"scope":"user"}"#).unwrap();
        assert!(req.agent_id.is_none());
    }
}
