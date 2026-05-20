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

/// Per-user synthesized summary path: `{username}/_synthesized/summary.md`.
///
/// A per-user summary folds both memory types together, so there is no
/// `memory_type` segment.
pub fn per_user_synthesis_path(username: &str) -> String {
    format!("{}/_synthesized/summary.md", username)
}

/// Global (cross-user) synthesized summary path: `_synthesized/summary.md`.
pub fn global_synthesis_path() -> String {
    "_synthesized/summary.md".to_string()
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
    fn per_user_synthesis_path_has_no_memory_type() {
        assert_eq!(
            super::per_user_synthesis_path("alice"),
            "alice/_synthesized/summary.md"
        );
    }

    #[test]
    fn global_synthesis_path_is_top_level() {
        assert_eq!(global_synthesis_path(), "_synthesized/summary.md");
    }
}
