use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// Memory sub-system configuration (storage + index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Root directory for Markdown memory files on disk.
    pub memory_dir: String,
    /// Path to the SQLite database file.
    pub db_path: String,
    /// Target line count per chunk when splitting content.
    pub chunk_size: usize,
    /// Overlap in lines between consecutive chunks.
    pub chunk_overlap: usize,
    /// Number of days before a memory's score decays to half its original value.
    pub temporal_decay_days: u32,
    /// Default hybrid search weight (`0.0` = pure keyword, `1.0` = pure vector).
    pub hybrid_weight: f32,
    /// Seconds to wait after the last `FileChanged` before the Synthesizer
    /// processes its pending set. Zero disables batching (process per event).
    #[serde(default = "default_synthesizer_cooldown_secs")]
    pub synthesizer_cooldown_secs: u64,
    /// Maximum size in bytes for a single synthesized output file before the
    /// writer rolls over to a suffixed name (`{date}-1.md`, `{date}-2.md`, …).
    #[serde(default = "default_synthesis_max_file_bytes")]
    pub synthesis_max_file_bytes: u64,
}

fn default_synthesizer_cooldown_secs() -> u64 {
    300
}

fn default_synthesis_max_file_bytes() -> u64 {
    1024 * 1024
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            memory_dir: "memory".to_string(),
            db_path: "memoryhub.db".to_string(),
            chunk_size: 400,
            chunk_overlap: 80,
            temporal_decay_days: 30,
            hybrid_weight: 0.5,
            synthesizer_cooldown_secs: 300,
            synthesis_max_file_bytes: 1024 * 1024,
        }
    }
}

impl MemoryConfig {
    /// Resolves `memory_dir` and `db_path` against the base directory.
    ///
    /// The SQLite `:memory:` sentinel and absolute paths are left unchanged; a `~/…` prefix
    /// expands to the home directory; any other relative path is joined onto `base`.
    pub fn resolve_paths(&mut self, base: &Path) -> Result<(), ConfigError> {
        self.memory_dir = resolve_path(base, &self.memory_dir)?;
        if self.db_path != ":memory:" {
            self.db_path = resolve_path(base, &self.db_path)?;
        }

        Ok(())
    }
}

/// Resolves a single configured path string against the base directory.
#[inline]
fn resolve_path(base: &Path, value: &str) -> Result<String, ConfigError> {
    if let Some(rest) = value.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
        return Ok(home.join(rest).to_string_lossy().to_string());
    }
    if Path::new(value).is_absolute() {
        return Ok(value.to_string());
    }
    Ok(base.join(value).to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let config = MemoryConfig::default();
        assert_eq!(config.memory_dir, "memory");
        assert_eq!(config.db_path, "memoryhub.db");
        assert_eq!(config.chunk_size, 400);
        assert_eq!(config.chunk_overlap, 80);
        assert_eq!(config.temporal_decay_days, 30);
        assert_eq!(config.hybrid_weight, 0.5);
        assert_eq!(config.synthesizer_cooldown_secs, 300);
        assert_eq!(config.synthesis_max_file_bytes, 1024 * 1024);
    }

    #[test]
    fn resolve_paths_joins_relative_paths_onto_base() {
        let base = Path::new("/data/mh");
        let mut config = MemoryConfig::default();
        config.resolve_paths(base).unwrap();

        assert_eq!(config.memory_dir, base.join("memory").to_string_lossy());
        assert_eq!(config.db_path, base.join("memoryhub.db").to_string_lossy());
    }

    #[test]
    fn resolve_paths_expands_tilde() {
        let home = dirs::home_dir().unwrap();
        let mut config = MemoryConfig {
            memory_dir: "~/mem".to_string(),
            db_path: "~/mh.db".to_string(),
            ..MemoryConfig::default()
        };
        config.resolve_paths(Path::new("/data/mh")).unwrap();

        assert!(!config.memory_dir.starts_with("~/"));
        assert_eq!(config.memory_dir, home.join("mem").to_string_lossy());
        assert_eq!(config.db_path, home.join("mh.db").to_string_lossy());
    }

    #[test]
    fn resolve_paths_leaves_absolute_paths_unchanged() {
        let (mem, db) = if cfg!(windows) {
            ("C:\\tmp\\memory", "C:\\tmp\\test.db")
        } else {
            ("/tmp/memory", "/tmp/test.db")
        };
        let mut config = MemoryConfig {
            memory_dir: mem.to_string(),
            db_path: db.to_string(),
            ..MemoryConfig::default()
        };
        config.resolve_paths(Path::new("/data/mh")).unwrap();

        assert_eq!(config.memory_dir, mem);
        assert_eq!(config.db_path, db);
    }

    #[test]
    fn resolve_paths_leaves_in_memory_db_unchanged() {
        let mut config = MemoryConfig {
            db_path: ":memory:".to_string(),
            ..MemoryConfig::default()
        };
        config.resolve_paths(Path::new("/data/mh")).unwrap();

        assert_eq!(config.db_path, ":memory:");
    }
}
