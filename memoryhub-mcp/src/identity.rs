//! Utilities for resolving the agent id.
//!
//! Each connecting client agent is identified by a UUID. Usually this is provided by the
//! `MEMORYHUB_AGENT_ID` environment variable. If the env variable is not set, a UUID is generated
//! and persisted on disk (identifyed by the client name) for future reuse.

use std::fs;
use std::io;
use std::path::Path;

use uuid::Uuid;

/// Reduces an MCP client name to a safe filename slug.
///
/// Only `[a-z0-9-_]` are kept, every other run collapsed to a single `-`. Empty input (or
/// all-invalid) yields `"default"`.
pub fn slug(name: Option<&str>) -> String {
    let raw = name.unwrap_or("").trim().to_lowercase();
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolves the `agent_id`.
///
/// If `agent_id` is provided, returns it. Otherwise reads-or-creates a UUID at
/// `base_dir/agents/<slug(client_name)>`, persisting a freshly generated one on first use.
pub fn resolve_agent_id(
    agent_id: Option<Uuid>,
    client_name: Option<&str>,
    base_dir: &Path,
) -> io::Result<Uuid> {
    if let Some(id) = agent_id {
        return Ok(id);
    }

    let dir = base_dir.join("agents");
    let path = dir.join(slug(client_name));

    if let Ok(existing) = fs::read_to_string(&path)
        && let Ok(id) = existing.trim().parse::<Uuid>()
    {
        return Ok(id);
    }

    let id = Uuid::new_v4();
    fs::create_dir_all(&dir)?;
    fs::write(&path, id.to_string())?;

    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_sanitizes_and_defaults() {
        assert_eq!(slug(Some("Claude Code")), "claude-code");
        assert_eq!(slug(Some("cursor")), "cursor");
        assert_eq!(slug(Some("  !!  ")), "default");
        assert_eq!(slug(None), "default");
        assert_eq!(slug(Some("a/../b")), "a-b");
    }

    #[test]
    fn override_wins() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let got = resolve_agent_id(Some(id), Some("cursor"), dir.path()).unwrap();
        assert_eq!(got, id);
        // Nothing persisted when overridden.
        assert!(!dir.path().join("agents").exists());
    }

    #[test]
    fn generates_then_reuses_per_client() {
        let dir = tempfile::tempdir().unwrap();
        let first = resolve_agent_id(None, Some("cursor"), dir.path()).unwrap();
        let again = resolve_agent_id(None, Some("cursor"), dir.path()).unwrap();
        assert_eq!(first, again, "same client reuses its persisted id");
        assert!(dir.path().join("agents/cursor").exists());
    }

    #[test]
    fn different_clients_get_different_ids() {
        let dir = tempfile::tempdir().unwrap();
        let cursor = resolve_agent_id(None, Some("cursor"), dir.path()).unwrap();
        let claude = resolve_agent_id(None, Some("claude-code"), dir.path()).unwrap();
        assert_ne!(cursor, claude);
    }

    #[test]
    fn missing_name_uses_default_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let id = resolve_agent_id(None, None, dir.path()).unwrap();
        let persisted = std::fs::read_to_string(dir.path().join("agents/default")).unwrap();
        assert_eq!(persisted.trim(), id.to_string());
    }
}
