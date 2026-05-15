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
}

fn default_synthesizer_cooldown_secs() -> u64 {
    300
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            memory_dir: "~/.clawchorus/memory".to_string(),
            db_path: "~/.clawchorus/clawchorus.db".to_string(),
            chunk_size: 400,
            chunk_overlap: 80,
            temporal_decay_days: 30,
            hybrid_weight: 0.5,
            synthesizer_cooldown_secs: 300,
        }
    }
}

impl MemoryConfig {
    /// Expand `~` prefixes in `memory_dir` and `db_path` to the user's home directory.
    pub fn resolve_paths(&mut self) -> Result<(), ConfigError> {
        let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
        if self.memory_dir.starts_with("~/") {
            self.memory_dir = home
                .join(&self.memory_dir[2..])
                .to_string_lossy()
                .to_string();
        }
        if self.db_path.starts_with("~/") {
            self.db_path = home.join(&self.db_path[2..]).to_string_lossy().to_string();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_paths_expands_tilde() {
        let home = dirs::home_dir().unwrap();
        let mut config = MemoryConfig::default();
        config.resolve_paths().unwrap();

        assert!(!config.memory_dir.starts_with("~/"));
        assert!(!config.db_path.starts_with("~/"));
        assert!(
            config
                .memory_dir
                .starts_with(home.to_string_lossy().as_ref())
        );
        assert!(config.memory_dir.ends_with(".clawchorus/memory"));
        assert!(config.db_path.ends_with(".clawchorus/clawchorus.db"));
    }

    #[test]
    fn default_synthesizer_cooldown_is_300s() {
        let config = MemoryConfig::default();
        assert_eq!(config.synthesizer_cooldown_secs, 300);
    }

    #[test]
    fn resolve_paths_leaves_absolute_paths_unchanged() {
        let mut config = MemoryConfig {
            memory_dir: "/tmp/memory".to_string(),
            db_path: "/tmp/test.db".to_string(),
            ..MemoryConfig::default()
        };
        config.resolve_paths().unwrap();

        assert_eq!(config.memory_dir, "/tmp/memory");
        assert_eq!(config.db_path, "/tmp/test.db");
    }
}
