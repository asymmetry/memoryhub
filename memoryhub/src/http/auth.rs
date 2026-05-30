//! Authentication storage and principals for the HTTP layer.
//!
//! `AuthStore` is a plain struct (not an actor) wrapping a SQLite connection to its own
//! `auth.db`. SQLite's own locking serializes access; blocking queries run in `spawn_blocking`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
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
}
