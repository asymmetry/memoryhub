//! Admin (`/v1/admin/*`) and self (`/v1/me`) handlers.

use axum::{Json, extract::Path, extract::State};
use serde::{Deserialize, Serialize};

use super::HttpServerState;
use super::error::HttpError;
use super::middleware::{AdminPrincipal, AuthUser};

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub username: String,
    pub role: String,
}

/// `GET /v1/me` — the calling user's own identity.
pub async fn me(user: AuthUser) -> Json<MeResponse> {
    Json(MeResponse {
        username: user.username,
        role: user.role,
    })
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "user".to_string()
}

#[derive(Debug, Serialize)]
pub struct UserView {
    pub username: String,
    pub role: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ListUsersResponse {
    pub users: Vec<UserView>,
}

pub async fn create_user(
    _admin: AdminPrincipal,
    State(state): State<HttpServerState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserView>, HttpError> {
    let info = state.auth.create_user(&req.username, &req.role).await?;
    Ok(Json(UserView {
        username: info.username,
        role: info.role,
        created_at: info.created_at,
    }))
}

pub async fn list_users(
    _admin: AdminPrincipal,
    State(state): State<HttpServerState>,
) -> Result<Json<ListUsersResponse>, HttpError> {
    let users = state
        .auth
        .list_users()
        .await?
        .into_iter()
        .map(|u| UserView {
            username: u.username,
            role: u.role,
            created_at: u.created_at,
        })
        .collect();
    Ok(Json(ListUsersResponse { users }))
}

pub async fn delete_user(
    _admin: AdminPrincipal,
    State(state): State<HttpServerState>,
    Path(username): Path<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    state.auth.delete_user(&username).await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct NewTokenResponse {
    pub id: String,
    pub token: String,
}

pub async fn create_token(
    _admin: AdminPrincipal,
    State(state): State<HttpServerState>,
    Path(username): Path<String>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<Json<NewTokenResponse>, HttpError> {
    let new = state
        .auth
        .create_token(&username, req.name.as_deref(), req.expires_at)
        .await?;
    Ok(Json(NewTokenResponse {
        id: new.id,
        token: new.secret,
    }))
}

#[derive(Debug, Serialize)]
pub struct TokenView {
    pub id: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListTokensResponse {
    pub tokens: Vec<TokenView>,
}

pub async fn list_tokens(
    _admin: AdminPrincipal,
    State(state): State<HttpServerState>,
    Path(username): Path<String>,
) -> Result<Json<ListTokensResponse>, HttpError> {
    let tokens = state
        .auth
        .list_tokens(&username)
        .await?
        .into_iter()
        .map(|t| TokenView {
            id: t.id,
            name: t.name,
            created_at: t.created_at,
            expires_at: t.expires_at,
        })
        .collect();
    Ok(Json(ListTokensResponse { tokens }))
}

pub async fn revoke_token(
    _admin: AdminPrincipal,
    State(state): State<HttpServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    state.auth.revoke_token(&id).await?;
    Ok(Json(serde_json::json!({})))
}
