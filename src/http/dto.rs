//! Request and response JSON structs for the HTTP API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::MemoryType;
use crate::memory::messages::SearchResult;

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
