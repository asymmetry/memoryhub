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
use rusqlite::{Connection, ErrorCode, params};
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
}
