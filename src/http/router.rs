//! Axum router construction for the HTTP API.

use acktor::Address;
use axum::Router;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

use crate::http::handlers::{delete, health, read, search, write};
use crate::memory::MemoryManager;

pub fn build_router(mm: Address<MemoryManager>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/memories/write", post(write))
        .route("/memories/read", post(read))
        .route("/memories/delete", post(delete))
        .route("/search", post(search))
        .layer(TraceLayer::new_for_http())
        .with_state(mm)
}
