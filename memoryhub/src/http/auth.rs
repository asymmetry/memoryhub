//! Authentication storage and principals for the HTTP layer.
//!
//! `AuthStore` is a plain struct (not an actor) wrapping a SQLite connection to its own
//! `auth.db`. SQLite's own locking serializes access; blocking queries run in `spawn_blocking`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use rand::RngExt;
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Prefix on every token secret; aids leak scanning.
const TOKEN_PREFIX: &str = "mh_";

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// The configured break-glass root admin token. Has no memory namespace.
    Root,
    /// A real user resolved from a token in `auth.db`.
    User { username: String, role: String },
}

/// The `[auth]` configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Path to the auth SQLite database (resolved against `base_dir` like other data paths).
    pub db_path: String,
    /// Optional root admin token; overridden by `MEMORYHUB_ADMIN_TOKEN`.
    #[serde(default)]
    pub admin_token: Option<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            db_path: "auth.db".to_string(),
            admin_token: None,
        }
    }
}

impl AuthConfig {
    /// Resolves `db_path` against the base directory (mirrors `MemoryConfig::resolve_paths`).
    pub fn resolve_paths(&mut self, base: &Path) {
        if self.db_path != ":memory:" && !Path::new(&self.db_path).is_absolute() {
            self.db_path = base.join(&self.db_path).to_string_lossy().to_string();
        }
    }

    /// Applies the `MEMORYHUB_ADMIN_TOKEN` env override (preferred over the config value).
    pub fn apply_env(&mut self) {
        if let Some(tok) = std::env::var_os("MEMORYHUB_ADMIN_TOKEN")
            && !tok.is_empty()
        {
            self.admin_token = Some(tok.to_string_lossy().to_string());
        }
    }
}

/// A user row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub username: String,
    pub role: String,
    pub created_at: i64,
}

/// A token row, without the secret (which is never stored or listed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    pub id: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

/// A freshly minted token; the `secret` is returned once and never persisted.
#[derive(Debug, Clone)]
pub struct NewToken {
    pub id: String,
    pub secret: String,
}

/// Errors from `AuthStore`.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("user already exists")]
    UserExists,

    #[error("user not found")]
    UserNotFound,

    #[error("token not found")]
    TokenNotFound,

    #[error("auth db error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("auth task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// SHA-256 hex digest of a secret. Used both to persist token hashes and to compare the root
/// token as fixed-length digests (avoids leaking secret length / early-exit on raw bytes).
fn sha256_hex(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// Generates a new token secret: `mh_` + base64url(32 random bytes, no padding).
fn generate_secret() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(bytes))
}

/// User + token storage over its own `auth.db`. Not an actor; SQLite locking serializes access.
#[derive(Clone)]
pub struct AuthStore {
    conn: Arc<Mutex<Connection>>,
    /// SHA-256 hex of the configured root admin token, if any.
    root_token_hash: Option<String>,
}

impl AuthStore {
    /// Opens (or creates) `auth.db` at `path`. `admin_token` is the optional root break-glass
    /// secret (its hash is retained for comparison; the secret itself is not stored on disk).
    pub fn open(path: &Path, admin_token: Option<String>) -> Result<Self, AuthError> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn, admin_token)
    }

    /// Opens an in-memory `auth.db` (tests).
    pub fn open_in_memory(admin_token: Option<String>) -> Result<Self, AuthError> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn, admin_token)
    }

    fn from_connection(conn: Connection, admin_token: Option<String>) -> Result<Self, AuthError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            root_token_hash: admin_token.as_deref().map(sha256_hex),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), AuthError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                username   TEXT PRIMARY KEY,
                role       TEXT NOT NULL DEFAULT 'user',
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tokens (
                id         TEXT PRIMARY KEY,
                username   TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
                token_hash TEXT NOT NULL UNIQUE,
                name       TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER
            );",
        )?;
        Ok(())
    }

    /// `true` if `secret` matches the configured root admin token (compared as digests).
    pub fn verify_root(&self, secret: &str) -> bool {
        match &self.root_token_hash {
            Some(expected) => &sha256_hex(secret) == expected,
            None => false,
        }
    }

    pub async fn create_user(&self, username: &str, role: &str) -> Result<UserInfo, AuthError> {
        let conn = Arc::clone(&self.conn);
        let username = username.to_string();
        let role = role.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let now = Utc::now().timestamp();
            let res = conn.execute(
                "INSERT INTO users (username, role, created_at) VALUES (?1, ?2, ?3)",
                params![username, role, now],
            );
            match res {
                Ok(_) => Ok(UserInfo {
                    username,
                    role,
                    created_at: now,
                }),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == ErrorCode::ConstraintViolation =>
                {
                    Err(AuthError::UserExists)
                }
                Err(e) => Err(e.into()),
            }
        })
        .await?
    }

    pub async fn list_users(&self) -> Result<Vec<UserInfo>, AuthError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt =
                conn.prepare("SELECT username, role, created_at FROM users ORDER BY username")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(UserInfo {
                        username: row.get(0)?,
                        role: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    pub async fn delete_user(&self, username: &str) -> Result<(), AuthError> {
        let conn = Arc::clone(&self.conn);
        let username = username.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let affected =
                conn.execute("DELETE FROM users WHERE username = ?1", params![username])?;
            if affected == 0 {
                Err(AuthError::UserNotFound)
            } else {
                Ok(())
            }
        })
        .await?
    }

    pub async fn has_admin(&self) -> Result<bool, AuthError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM users WHERE role = 'admin'",
                [],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
        .await?
    }

    pub async fn create_token(
        &self,
        username: &str,
        name: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<NewToken, AuthError> {
        let conn = Arc::clone(&self.conn);
        let username = username.to_string();
        let name = name.map(|n| n.to_string());
        let secret = generate_secret();
        let token_hash = sha256_hex(&secret);
        let id = uuid::Uuid::new_v4().to_string();
        let id_out = id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();

            // Explicit existence check so a missing user is a clean UserNotFound rather than an
            // opaque foreign-key constraint failure.
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM users WHERE username = ?1",
                    params![username],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !exists {
                return Err(AuthError::UserNotFound);
            }

            let now = Utc::now().timestamp();
            conn.execute(
                "INSERT INTO tokens (id, username, token_hash, name, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, username, token_hash, name, now, expires_at],
            )?;
            Ok(())
        })
        .await??;

        Ok(NewToken { id: id_out, secret })
    }

    pub async fn list_tokens(&self, username: &str) -> Result<Vec<TokenInfo>, AuthError> {
        let conn = Arc::clone(&self.conn);
        let username = username.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, name, created_at, expires_at FROM tokens
                 WHERE username = ?1 ORDER BY created_at",
            )?;
            let rows = stmt
                .query_map(params![username], |row| {
                    Ok(TokenInfo {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        created_at: row.get(2)?,
                        expires_at: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    pub async fn revoke_token(&self, id: &str) -> Result<(), AuthError> {
        let conn = Arc::clone(&self.conn);
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let affected = conn.execute("DELETE FROM tokens WHERE id = ?1", params![id])?;
            if affected == 0 {
                Err(AuthError::TokenNotFound)
            } else {
                Ok(())
            }
        })
        .await?
    }

    /// Resolves a token secret to a `Principal::User`, or `None` if unknown or expired.
    pub async fn resolve_token(&self, secret: &str) -> Result<Option<Principal>, AuthError> {
        let conn = Arc::clone(&self.conn);
        let token_hash = sha256_hex(secret);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let row: Option<(String, String, Option<i64>)> = conn
                .query_row(
                    "SELECT t.username, u.role, t.expires_at
                     FROM tokens t JOIN users u ON u.username = t.username
                     WHERE t.token_hash = ?1",
                    params![token_hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;

            let Some((username, role, expires_at)) = row else {
                return Ok(None);
            };
            if let Some(exp) = expires_at
                && exp < Utc::now().timestamp()
            {
                return Ok(None);
            }
            Ok(Some(Principal::User { username, role }))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secret_has_prefix_and_is_unique() {
        let a = generate_secret();
        let b = generate_secret();
        assert!(a.starts_with("mh_"));
        assert!(b.starts_with("mh_"));
        assert_ne!(a, b);
        // 32 bytes base64url-nopad = 43 chars, plus "mh_".
        assert_eq!(a.len(), 3 + 43);
    }

    #[test]
    fn sha256_hex_is_stable_and_64_chars() {
        let h = sha256_hex("mh_example");
        assert_eq!(h.len(), 64);
        assert_eq!(h, sha256_hex("mh_example"));
        assert_ne!(h, sha256_hex("mh_other"));
    }

    fn store() -> AuthStore {
        AuthStore::open_in_memory(None).unwrap()
    }

    #[tokio::test]
    async fn create_and_list_users() {
        let s = store();
        let info = s.create_user("alice", "user").await.unwrap();
        assert_eq!(info.username, "alice");
        assert_eq!(info.role, "user");

        let users = s.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "alice");
    }

    #[tokio::test]
    async fn create_duplicate_user_is_conflict() {
        let s = store();
        s.create_user("alice", "user").await.unwrap();
        let err = s.create_user("alice", "admin").await.unwrap_err();
        assert!(matches!(err, AuthError::UserExists));
    }

    #[tokio::test]
    async fn delete_user_then_missing() {
        let s = store();
        s.create_user("alice", "user").await.unwrap();
        s.delete_user("alice").await.unwrap();
        assert!(s.list_users().await.unwrap().is_empty());

        let err = s.delete_user("alice").await.unwrap_err();
        assert!(matches!(err, AuthError::UserNotFound));
    }

    #[tokio::test]
    async fn has_admin_reflects_roles() {
        let s = store();
        assert!(!s.has_admin().await.unwrap());
        s.create_user("alice", "user").await.unwrap();
        assert!(!s.has_admin().await.unwrap());
        s.create_user("bob", "admin").await.unwrap();
        assert!(s.has_admin().await.unwrap());
    }

    #[tokio::test]
    async fn mint_resolve_list_revoke_token() {
        let s = store();
        s.create_user("alice", "user").await.unwrap();

        let minted = s.create_token("alice", Some("laptop"), None).await.unwrap();
        assert!(minted.secret.starts_with("mh_"));

        // Resolve the secret back to the user.
        let principal = s.resolve_token(&minted.secret).await.unwrap();
        assert_eq!(
            principal,
            Some(Principal::User {
                username: "alice".into(),
                role: "user".into()
            })
        );

        // The secret is never listed; only metadata.
        let tokens = s.list_tokens("alice").await.unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, minted.id);
        assert_eq!(tokens[0].name.as_deref(), Some("laptop"));

        // Revoke removes it.
        s.revoke_token(&minted.id).await.unwrap();
        assert!(s.resolve_token(&minted.secret).await.unwrap().is_none());
        let err = s.revoke_token(&minted.id).await.unwrap_err();
        assert!(matches!(err, AuthError::TokenNotFound));
    }

    #[tokio::test]
    async fn unknown_and_expired_tokens_do_not_resolve() {
        let s = store();
        s.create_user("alice", "user").await.unwrap();

        assert!(s.resolve_token("mh_nope").await.unwrap().is_none());

        let past = Utc::now().timestamp() - 60;
        let minted = s.create_token("alice", None, Some(past)).await.unwrap();
        assert!(s.resolve_token(&minted.secret).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn token_for_missing_user_is_user_not_found() {
        let s = store();
        let err = s.create_token("ghost", None, None).await.unwrap_err();
        assert!(matches!(err, AuthError::UserNotFound));
    }

    #[tokio::test]
    async fn deleting_user_cascades_tokens() {
        let s = store();
        s.create_user("alice", "user").await.unwrap();
        let minted = s.create_token("alice", None, None).await.unwrap();
        s.delete_user("alice").await.unwrap();
        // Cascade removed the token, so it no longer resolves.
        assert!(s.resolve_token(&minted.secret).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn verify_root_matches_only_configured_token() {
        let s = AuthStore::open_in_memory(Some("mh_root_secret".into())).unwrap();
        assert!(s.verify_root("mh_root_secret"));
        assert!(!s.verify_root("mh_wrong"));

        let no_root = store();
        assert!(!no_root.verify_root("mh_anything"));
    }

    #[test]
    fn auth_config_resolves_relative_db_path() {
        let mut cfg = AuthConfig::default();
        cfg.resolve_paths(Path::new("/data/mh"));
        assert_eq!(cfg.db_path, "/data/mh/auth.db");
    }

    #[test]
    fn auth_config_leaves_in_memory_unchanged() {
        let mut cfg = AuthConfig {
            db_path: ":memory:".into(),
            admin_token: None,
        };
        cfg.resolve_paths(Path::new("/data/mh"));
        assert_eq!(cfg.db_path, ":memory:");
    }
}
