//! Path derivation utilities for the Memory Manager.
//!
//! Raw layout: `{username}/{agent_id}/{project}/{filename}`. `project` defaults to `_default`
//! when omitted. The agent embeds any sub-path directly in `filename` by flattening every `/` and
//! `\` in it to `_` so the result is a single path segment.
//!
//! Synthesized layout: `[{username}/[{agent_id}/]]_synthesized/YYYY-MM-DD-{n}.md` — per-agent,
//! per-user, and cross-user (global) targets respectively. Each synthesis run writes to the
//! greatest existing file (ordered by date then numeric suffix) in the target folder, or creates a
//! fresh `{today}-01.md`, and rolls over to the next suffix when the current file is already at or
//! above the configured size cap.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use tokio::fs;
use uuid::Uuid;

use super::error::MemoryError;
use crate::llm::SynthesisTarget;

/// Project bucket used when the caller omits `project`.
const DEFAULT_PROJECT: &str = "_default";

/// Reserved segment names a caller may not use as a project.
const RESERVED_PROJECTS: [&str; 2] = ["_synthesized", DEFAULT_PROJECT];

/// Validate and resolve a caller-supplied project into its path segment.
///
/// `None`/empty → `_default`. Rejects a value containing `/` or `\`, or equal
/// to a reserved name.
pub fn resolve_project(project: Option<&str>) -> Result<String, MemoryError> {
    match project.map(str::trim) {
        None | Some("") => Ok(DEFAULT_PROJECT.to_string()),
        Some(p) if p.contains('/') || p.contains('\\') => Err(MemoryError::InvalidProject(
            format!("project must be a single path segment, got {}", p),
        )),
        Some(p) if RESERVED_PROJECTS.contains(&p) => Err(MemoryError::InvalidProject(format!(
            "project name {} is reserved",
            p
        ))),
        Some(p) => Ok(p.to_string()),
    }
}

/// Derive the storage path for a raw memory file:
/// `{username}/{agent_id}/{project}/{flattened_filename}`.
///
/// `project` is validated via [`resolve_project`]; `filename`'s `/` and `\` are
/// flattened to `_`.
pub fn get_raw_path(
    username: &str,
    agent_id: Uuid,
    project: Option<&str>,
    filename: &str,
) -> Result<String, MemoryError> {
    let project = resolve_project(project)?;
    Ok(format!(
        "{}/{}/{}/{}",
        username,
        agent_id,
        project,
        filename.replace(['/', '\\'], "_")
    ))
}

/// Relative folder (under `memory_dir`) where synthesized files for `target` live.
#[inline]
fn synthesis_folder(target: &SynthesisTarget) -> String {
    match target {
        SynthesisTarget::Agent { username, agent_id } => {
            format!("{}/{}/_synthesized", username, agent_id)
        }
        SynthesisTarget::User { username } => format!("{}/_synthesized", username),
        SynthesisTarget::Global => "_synthesized".to_string(),
    }
}

/// Return the relative path the synthesizer should write to next for `target`.
///
/// Picks the lexicographically greatest existing `{date}-{n}.md` in the target folder. If that
/// file's on-disk size is at or above `max_file_bytes`, rolls over to the next suffixed file. If
/// no synthesized file exists yet, returns `{today}-01.md`.
pub async fn current_synthesis_path(
    memory_dir: &Path,
    target: &SynthesisTarget,
    today: NaiveDate,
    max_size: u64,
) -> String {
    let folder_rel = synthesis_folder(target);
    let folder_abs = memory_dir.join(&folder_rel);
    let today_str = today.format("%Y-%m-%d").to_string();

    let candidate = match find_latest_synthesis_file(&folder_abs).await {
        Some((name, size)) if size < max_size => name,
        Some((name, _)) => next_filename(&name, &today_str),
        None => format!("{}-01.md", today_str),
    };

    format!("{}/{}", folder_rel, candidate)
}

/// Return the relative path of the most recently written synthesized file for `target`, or `None`
/// if no synthesized file exists yet.
pub async fn get_latest_synthesis_file(
    memory_dir: &Path,
    target: &SynthesisTarget,
) -> Option<String> {
    let folder_rel = synthesis_folder(target);
    let folder_abs = memory_dir.join(&folder_rel);
    find_latest_synthesis_file(&folder_abs)
        .await
        .map(|(name, _)| format!("{}/{}", folder_rel, name))
}

/// Returns `(filename, size)` for the latest synthesis file.
async fn find_latest_synthesis_file(folder_abs: &Path) -> Option<(String, u64)> {
    let mut entries = fs::read_dir(folder_abs).await.ok()?;

    let mut best: Option<(String, PathBuf, (String, u32))> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(key) = parse_filename(name) else {
            continue;
        };
        match &best {
            Some((_, _, k)) if &key <= k => {}
            _ => best = Some((name.to_string(), path, key)),
        }
    }

    let (name, path, _) = best?;
    let size = fs::metadata(&path).await.ok()?.len();

    Some((name, size))
}

/// Parses `YYYY-MM-DD-N.md` into a `(date, suffix)` key suitable for ordering. Returns `None` for
/// any other name.
#[inline]
fn parse_filename(name: &str) -> Option<(String, u32)> {
    let stem = name.strip_suffix(".md")?;
    let mut parts = stem.splitn(4, '-');
    let y = parts.next()?;
    let m = parts.next()?;
    let d = parts.next()?;
    let n = parts.next()?;
    if y.len() != 4 || !y.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if m.len() != 2 || !m.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if d.len() != 2 || !d.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let date = format!("{}-{}-{}", y, m, d);
    let suffix = n.parse().ok()?;

    Some((date, suffix))
}

/// Given the current synthesis file (e.g. `2026-05-20-02.md`), return the next
/// valid filename.
///
/// If the existing file is from an earlier date, jump to `{today}-1.md`.
/// Otherwise increment the trailing `-N` counter.
#[inline]
fn next_filename(current: &str, today: &str) -> String {
    let Some((date, suffix)) = parse_filename(current) else {
        return format!("{}-01.md", today);
    };
    if date != today {
        return format!("{}-01.md", today);
    }
    format!("{}-{:02}.md", today, suffix.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::error::MemoryError;
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn simple_filename_path() {
        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = get_raw_path("alice", agent_id, Some("notes"), "2026-03-31.md").unwrap();
        assert_eq!(
            path,
            "alice/550e8400-e29b-41d4-a716-446655440000/notes/2026-03-31.md"
        );
    }

    #[test]
    fn filename_with_slash_is_flattened_and_default_project() {
        let agent_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = get_raw_path("alice", agent_id, None, "notes/2026-03-31.md").unwrap();
        assert_eq!(
            path,
            "alice/550e8400-e29b-41d4-a716-446655440000/_default/notes_2026-03-31.md"
        );
        let path = get_raw_path("alice", agent_id, None, "notes\\2026-03-31.md").unwrap();
        assert_eq!(
            path,
            "alice/550e8400-e29b-41d4-a716-446655440000/_default/notes_2026-03-31.md"
        );
    }

    #[tokio::test]
    async fn synthesis_path_for_agent_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = current_synthesis_path(
            dir.path(),
            &SynthesisTarget::Agent {
                username: "alice".into(),
                agent_id: "agent1".into(),
            },
            date(2026, 5, 20),
            1_048_576,
        )
        .await;
        assert_eq!(path, "alice/agent1/_synthesized/2026-05-20-01.md");
    }

    #[tokio::test]
    async fn synthesis_path_uses_date_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = current_synthesis_path(
            dir.path(),
            &SynthesisTarget::User {
                username: "alice".into(),
            },
            date(2026, 5, 20),
            1_048_576,
        )
        .await;
        assert_eq!(path, "alice/_synthesized/2026-05-20-01.md");

        let path = current_synthesis_path(
            dir.path(),
            &SynthesisTarget::Global,
            date(2026, 5, 20),
            1024,
        )
        .await;
        assert_eq!(path, "_synthesized/2026-05-20-01.md");
    }

    #[tokio::test]
    async fn synthesis_path_reuses_existing_dated_file_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("alice").join("_synthesized");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("2026-05-20-01.md"), b"short").unwrap();

        let path = current_synthesis_path(
            dir.path(),
            &SynthesisTarget::User {
                username: "alice".into(),
            },
            date(2026, 5, 20),
            1024,
        )
        .await;
        assert_eq!(path, "alice/_synthesized/2026-05-20-01.md");
    }

    #[tokio::test]
    async fn synthesis_path_rolls_over_increments_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("alice").join("_synthesized");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("2026-05-20-01.md"), vec![b'x'; 1024]).unwrap();
        fs::write(folder.join("2026-05-20-02.md"), vec![b'x'; 1024]).unwrap();

        let path = current_synthesis_path(
            dir.path(),
            &SynthesisTarget::User {
                username: "alice".into(),
            },
            date(2026, 5, 20),
            1024,
        )
        .await;
        assert_eq!(path, "alice/_synthesized/2026-05-20-03.md");
    }

    #[tokio::test]
    async fn synthesis_path_starts_fresh_on_new_date() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("alice").join("_synthesized");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("2026-05-19-01.md"), vec![b'x'; 1024]).unwrap();

        let path = current_synthesis_path(
            dir.path(),
            &SynthesisTarget::User {
                username: "alice".into(),
            },
            date(2026, 5, 20),
            1024,
        )
        .await;
        assert_eq!(path, "alice/_synthesized/2026-05-20-01.md");
    }

    #[tokio::test]
    async fn latest_synthesis_path_picks_lexicographic_max() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("alice").join("_synthesized");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("2026-05-19-01.md"), b"a").unwrap();
        fs::write(folder.join("2026-05-20-01.md"), b"b").unwrap();
        fs::write(folder.join("2026-05-20-02.md"), b"c").unwrap();

        let path = get_latest_synthesis_file(
            dir.path(),
            &SynthesisTarget::User {
                username: "alice".into(),
            },
        )
        .await;
        assert_eq!(path.as_deref(), Some("alice/_synthesized/2026-05-20-02.md"));
    }

    #[tokio::test]
    async fn latest_synthesis_path_returns_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = get_latest_synthesis_file(
            dir.path(),
            &SynthesisTarget::User {
                username: "alice".into(),
            },
        )
        .await;
        assert!(path.is_none());
    }

    #[test]
    fn resolve_project_defaults_when_absent() {
        assert_eq!(resolve_project(None).unwrap(), "_default");
        assert_eq!(resolve_project(Some("")).unwrap(), "_default");
    }

    #[test]
    fn resolve_project_accepts_plain_name() {
        assert_eq!(resolve_project(Some("memoryhub")).unwrap(), "memoryhub");
    }

    #[test]
    fn resolve_project_rejects_separators_and_reserved() {
        assert!(matches!(
            resolve_project(Some("a/b")),
            Err(MemoryError::InvalidProject(_))
        ));
        assert!(matches!(
            resolve_project(Some("a\\b")),
            Err(MemoryError::InvalidProject(_))
        ));
        assert!(matches!(
            resolve_project(Some("_synthesized")),
            Err(MemoryError::InvalidProject(_))
        ));
        assert!(matches!(
            resolve_project(Some("_default")),
            Err(MemoryError::InvalidProject(_))
        ));
    }
}
