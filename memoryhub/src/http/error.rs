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
    #[error("invalid username: {0}")]
    InvalidUsername(String),

    #[error("invalid role: {0}")]
    InvalidRole(String),

    #[error("user already exists")]
    UserExists,

    #[error("user not found")]
    UserNotFound,

    #[error("cannot delete the last admin user")]
    LastAdmin,

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

    #[error("bad request: {0}")]
    BadRequest(String),

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
            AuthError::InvalidUsername(msg) | AuthError::InvalidRole(msg) => {
                HttpError::BadRequest(msg)
            }
            AuthError::UserExists => HttpError::Conflict,
            AuthError::LastAdmin => HttpError::BadRequest(e.to_string()),
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
            HttpError::BadRequest(msg) => {
                (StatusCode::BAD_REQUEST, "bad_request", Some(msg.clone()))
            }
            HttpError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                Some(msg.clone()),
            ),
            HttpError::Memory(MemoryError::InvalidProject(msg)) => {
                (StatusCode::BAD_REQUEST, "bad_request", Some(msg.clone()))
            }
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

    /// Expectation for the response body's optional `message` field.
    enum Msg<'a> {
        /// `message` must be absent (the `skip_serializing_if` contract for message-less codes).
        Absent,
        /// `message` is present and contains this substring.
        Has(&'a str),
        /// `message` is present; its content is not checked.
        Present,
    }

    async fn assert_response(err: HttpError, status: StatusCode, code: &str, msg: Msg<'_>) {
        let resp = err.into_response();
        assert_eq!(resp.status(), status, "status for {code}");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            body.contains(&format!(r#""error":"{code}""#)),
            "code in {body}"
        );
        match msg {
            Msg::Absent => assert!(!body.contains("message"), "unexpected message in {body}"),
            Msg::Has(m) => assert!(body.contains(m), "expected {m:?} in {body}"),
            Msg::Present => assert!(body.contains("message"), "expected a message in {body}"),
        }
    }

    #[tokio::test]
    async fn http_errors_map_to_status_and_body() {
        use Msg::*;
        assert_response(
            HttpError::NotFound,
            StatusCode::NOT_FOUND,
            "not_found",
            Absent,
        )
        .await;
        assert_response(
            HttpError::Unauthorized,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            Absent,
        )
        .await;
        assert_response(
            HttpError::Forbidden,
            StatusCode::FORBIDDEN,
            "forbidden",
            Absent,
        )
        .await;
        assert_response(
            HttpError::Conflict,
            StatusCode::CONFLICT,
            "conflict",
            Absent,
        )
        .await;
        assert_response(
            HttpError::Unavailable("dead".into()),
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            Absent,
        )
        .await;
        assert_response(
            HttpError::Memory(MemoryError::SendError("boom".into())),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            Has("boom"),
        )
        .await;
        assert_response(
            HttpError::Memory(MemoryError::InvalidProject("bad".into())),
            StatusCode::BAD_REQUEST,
            "bad_request",
            Has("bad"),
        )
        .await;
    }

    #[tokio::test]
    async fn auth_errors_convert_to_http() {
        use Msg::*;
        assert_response(
            AuthError::UserExists.into(),
            StatusCode::CONFLICT,
            "conflict",
            Absent,
        )
        .await;
        assert_response(
            AuthError::InvalidUsername("a/b".into()).into(),
            StatusCode::BAD_REQUEST,
            "bad_request",
            Has("a/b"),
        )
        .await;
        assert_response(
            AuthError::UserNotFound.into(),
            StatusCode::NOT_FOUND,
            "not_found",
            Absent,
        )
        .await;
        assert_response(
            AuthError::LastAdmin.into(),
            StatusCode::BAD_REQUEST,
            "bad_request",
            Has("last admin"),
        )
        .await;
        assert_response(
            AuthError::Db(rusqlite::Error::QueryReturnedNoRows).into(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            Present,
        )
        .await;
    }
}
