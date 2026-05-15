//! Path derivation utilities for the Memory Manager.
//!
//! Layout: `{username}/{agent_id}/{memory_type}/{filename}`

use uuid::Uuid;

use crate::memory::MemoryType;

/// Derive the relative filesystem path for a memory file.
pub fn derive_rel_path(
    username: &str,
    agent_id: Uuid,
    memory_type: MemoryType,
    filename: &str,
) -> String {
    let type_dir = match memory_type {
        MemoryType::DailyNote => "daily_note",
        MemoryType::LongTerm => "long_term",
    };
    format!("{}/{}/{}/{}", username, agent_id, type_dir, filename)
}

/// Derive the relative filesystem path for a synthesized memory file.
///
/// `username = Some(...)` → `{username}/_synthesized/{memory_type}/{filename}`
/// `username = None`       → `_synthesized/{memory_type}/{filename}`
pub fn derive_synthesis_path(
    username: Option<&str>,
    memory_type: MemoryType,
    filename: &str,
) -> String {
    let type_dir = match memory_type {
        MemoryType::DailyNote => "daily_note",
        MemoryType::LongTerm => "long_term",
    };
    match username {
        Some(u) => format!("{}/_synthesized/{}/{}", u, type_dir, filename),
        None => format!("_synthesized/{}/{}", type_dir, filename),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn daily_note_path() {
        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = derive_rel_path("alice", agent_id, MemoryType::DailyNote, "2026-03-31.md");
        assert_eq!(
            path,
            "alice/550e8400-e29b-41d4-a716-446655440000/daily_note/2026-03-31.md"
        );
    }

    #[test]
    fn long_term_path() {
        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = derive_rel_path("bob", agent_id, MemoryType::LongTerm, "MEMORY.md");
        assert_eq!(
            path,
            "bob/550e8400-e29b-41d4-a716-446655440000/long_term/MEMORY.md"
        );
    }

    #[test]
    fn per_user_synthesis_path() {
        let path = derive_synthesis_path(Some("alice"), MemoryType::DailyNote, "2026-05-13.md");
        assert_eq!(path, "alice/_synthesized/daily_note/2026-05-13.md");
    }

    #[test]
    fn cross_user_synthesis_path() {
        let path = derive_synthesis_path(None, MemoryType::LongTerm, "merged.md");
        assert_eq!(path, "_synthesized/long_term/merged.md");
    }
}
