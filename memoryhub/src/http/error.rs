//! HTTP error type and response mapping.

use std::io;

use acktor::ErrorReport;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

use crate::memory::error::MemoryError;

/// Error type returned by the [`HttpServer`][super::HttpServer] actor itself (bind/listen
/// errors).
#[derive(Debug, Error)]
pub enum HttpServerError {
    #[error("invalid bind address {addr}")]
    InvalidAddr {
        addr: String,
        source: std::net::AddrParseError,
    },

    #[error("could not bind to {addr}")]
    Bind { addr: String, source: io::Error },

    #[error("could not create the auth store")]
    Auth {
        #[from]
        source: AuthError,
    },
}

/// Errors from the [`AuthStore`][super::AuthStore].
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("user already exists")]
    UserExists,

    #[error("user not found")]
    UserNotFound,

    #[error("token not found")]
    TokenNotFound,

    #[error(transparent)]
    Db(#[from] rusqlite::Error),

    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

/// Error type returned by HTTP handlers.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("not found")]
    NotFound,

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict")]
    Conflict,

    #[error("internal error: {0}")]
    Internal(String),

    #[error(transparent)]
    Memory(#[from] MemoryError),

    #[error("service unavailable: {0}")]
    Unavailable(String),
}

impl From<AuthError> for HttpError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::UserExists => HttpError::Conflict,
            AuthError::UserNotFound | AuthError::TokenNotFound => HttpError::NotFound,
            AuthError::Db(_) | AuthError::Join(_) => HttpError::Internal(e.report()),
        }
    }
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
            HttpError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", None),
            HttpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", None),
            HttpError::Conflict => (StatusCode::CONFLICT, "conflict", None),
            HttpError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                Some(msg.clone()),
            ),
            HttpError::Memory(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                Some(e.report()),
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
    use axum::body::to_bytes;

    use super::*;

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
        let err = HttpError::Memory(MemoryError::SendError("boom".into()));
        let (status, body) = body_string(err.into_response()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body.contains(r#""error":"internal""#));
        assert!(body.contains("boom"));
    }

    #[tokio::test]
    async fn unauthorized_maps_to_401() {
        let (status, body) = body_string(HttpError::Unauthorized.into_response()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, r#"{"error":"unauthorized"}"#);
    }

    #[tokio::test]
    async fn forbidden_maps_to_403() {
        let (status, body) = body_string(HttpError::Forbidden.into_response()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, r#"{"error":"forbidden"}"#);
    }

    #[tokio::test]
    async fn auth_user_exists_maps_to_409() {
        let err: HttpError = AuthError::UserExists.into();
        let (status, body) = body_string(err.into_response()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, r#"{"error":"conflict"}"#);
    }

    #[tokio::test]
    async fn auth_user_not_found_maps_to_404() {
        let err: HttpError = AuthError::UserNotFound.into();
        let (status, body) = body_string(err.into_response()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, r#"{"error":"not_found"}"#);
    }

    #[tokio::test]
    async fn auth_db_error_maps_to_500() {
        let err: HttpError = AuthError::Db(rusqlite::Error::QueryReturnedNoRows).into();
        let (status, body) = body_string(err.into_response()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body.contains(r#""error":"internal""#));
    }
}
