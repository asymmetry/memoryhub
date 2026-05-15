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
