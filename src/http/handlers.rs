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
        memory_type: req.memory_type,
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
        memory_type: req.memory_type,
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
        memory_type: req.memory_type,
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
