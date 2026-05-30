//! Authentication middleware and request extractors.
//!
//! `auth_middleware` runs on every `/v1` route except `/v1/health`: it resolves the bearer
//! token to a [`Principal`] and stores it in request extensions. The [`AuthUser`] and
//! [`AdminPrincipal`] extractors then read it.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;

use super::HttpServerState;
use super::auth::Principal;
use super::error::HttpError;

/// An authenticated real user (never `Principal::Root`).
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub username: String,
    pub role: String,
}

/// Marker extractor that succeeds only for an admin caller (`Root` or `role == "admin"`).
#[derive(Debug, Clone)]
pub struct AdminPrincipal;

/// Extracts the bearer secret from an `Authorization: Bearer <secret>` header.
fn bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

/// Middleware: resolve the bearer token to a [`Principal`] and insert it into extensions.
pub async fn auth_middleware(
    State(state): State<HttpServerState>,
    mut req: Request,
    next: Next,
) -> Result<Response, HttpError> {
    let secret = bearer(req.headers()).ok_or(HttpError::Unauthorized)?;

    let principal = if state.auth.verify_root(&secret) {
        Principal::Root
    } else {
        state
            .auth
            .resolve_token(&secret)
            .await?
            .ok_or(HttpError::Unauthorized)?
    };

    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<Principal>() {
            Some(Principal::User { username, role }) => Ok(AuthUser {
                username: username.clone(),
                role: role.clone(),
            }),
            // Root has no memory namespace; reject it for user-scoped routes.
            Some(Principal::Root) => Err(HttpError::Forbidden),
            None => Err(HttpError::Unauthorized),
        }
    }
}

impl<S> FromRequestParts<S> for AdminPrincipal
where
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<Principal>() {
            Some(Principal::Root) => Ok(AdminPrincipal),
            Some(Principal::User { role, .. }) if role == "admin" => Ok(AdminPrincipal),
            Some(Principal::User { .. }) => Err(HttpError::Forbidden),
            None => Err(HttpError::Unauthorized),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::{Router, middleware::from_fn_with_state};
    use tower::ServiceExt;

    use super::*;
    use crate::http::auth::AuthStore;

    async fn protected_ok(_user: AuthUser) -> StatusCode {
        StatusCode::OK
    }

    async fn admin_ok(_admin: AdminPrincipal) -> StatusCode {
        StatusCode::OK
    }

    // A standalone router for extractor tests, stating on `Arc<AuthStore>` directly so we don't
    // need to build a `MemoryManager`. It mirrors how the real router applies the auth layer.
    fn app(store: Arc<AuthStore>) -> Router {
        Router::new()
            .route("/user", get(protected_ok))
            .route("/admin", get(admin_ok))
            .route_layer(from_fn_with_state(store.clone(), bearer_layer))
            .with_state(store)
    }

    // Test-only middleware mirroring `auth_middleware` but stating on `Arc<AuthStore>` directly.
    async fn bearer_layer(
        State(store): State<Arc<AuthStore>>,
        mut req: Request<Body>,
        next: Next,
    ) -> Result<Response, HttpError> {
        let secret = bearer(req.headers()).ok_or(HttpError::Unauthorized)?;
        let principal = if store.verify_root(&secret) {
            Principal::Root
        } else {
            store
                .resolve_token(&secret)
                .await?
                .ok_or(HttpError::Unauthorized)?
        };
        req.extensions_mut().insert(principal);
        Ok(next.run(req).await)
    }

    async fn setup() -> (Arc<AuthStore>, String, String) {
        let store = Arc::new(AuthStore::open_in_memory(Some("mh_root".into())).unwrap());
        store.create_user("alice", "user").await.unwrap();
        store.create_user("admin", "admin").await.unwrap();
        let user_token = store
            .create_token("alice", None, None)
            .await
            .unwrap()
            .secret;
        let admin_token = store
            .create_token("admin", None, None)
            .await
            .unwrap()
            .secret;
        (store, user_token, admin_token)
    }

    fn req(uri: &str, auth: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().uri(uri);
        if let Some(a) = auth {
            b = b.header("authorization", format!("Bearer {}", a));
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn missing_token_is_401() {
        let (store, _, _) = setup().await;
        let resp = app(store).oneshot(req("/user", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bad_token_is_401() {
        let (store, _, _) = setup().await;
        let resp = app(store)
            .oneshot(req("/user", Some("mh_bad")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn user_token_reaches_user_route() {
        let (store, user, _) = setup().await;
        let resp = app(store).oneshot(req("/user", Some(&user))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn user_token_rejected_by_admin_route() {
        let (store, user, _) = setup().await;
        let resp = app(store)
            .oneshot(req("/admin", Some(&user)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_token_reaches_admin_route() {
        let (store, _, admin) = setup().await;
        let resp = app(store)
            .oneshot(req("/admin", Some(&admin)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn root_token_reaches_admin_but_not_user_route() {
        let (store, _, _) = setup().await;
        let resp = app(store.clone())
            .oneshot(req("/admin", Some("mh_root")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app(store)
            .oneshot(req("/user", Some("mh_root")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
