use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
pub use crate::llm::LlmConfig;
pub use crate::memory::config::MemoryConfig;

/// Full application configuration loaded from `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

/// HTTP server bind settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
        }
    }
}

/// Background synthesis agent settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Whether the synthesis agent loop should run.
    pub enabled: bool,
    /// How often (in seconds) the agent wakes to look for new syntheses.
    pub interval_secs: u64,
    /// Minimum cosine similarity required to merge two memories.
    pub similarity_threshold: f32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            similarity_threshold: 0.75,
        }
    }
}

/// Returns the base directory for ClawChorus data: `~/.clawchorus`.
pub fn base_dir() -> Result<PathBuf, ConfigError> {
    let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
    Ok(home.join(".clawchorus"))
}

/// Returns the default config file path: `~/.clawchorus/config.toml`.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(base_dir()?.join("config.toml"))
}

impl Config {
    /// Loads configuration from a TOML file at `path`.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.to_path_buf(),
            source: e,
        })?;
        let config: Config = toml::from_str(&raw).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            source: e.into(),
        })?;

        Ok(config)
    }

    /// Loads configuration from `~/.clawchorus/config.toml`, falling back to all defaults
    /// if the file does not exist.
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_path()?;
        let mut config = if path.exists() {
            Self::from_file(path)?
        } else {
            tracing::warn!("~/.clawchorus/config.toml not found — using built-in defaults");
            Self::default()
        };
        config.memory.resolve_paths()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn test_config_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join("config.toml")
    }

    #[test]
    fn test_load_from_file() {
        let config = Config::from_file(test_config_path()).unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.memory.memory_dir, "./test_store");
        assert_eq!(config.memory.db_path, ":memory:");
        assert!(!config.agent.enabled);
        assert_eq!(config.agent.interval_secs, 60);
    }

    #[test]
    fn test_load_from_missing_file() {
        let result = Config::from_file("nonexistent.toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.memory.memory_dir, "~/.clawchorus/memory");
        assert_eq!(config.memory.db_path, "~/.clawchorus/clawchorus.db");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
        assert_eq!(config.llm.provider, "deepseek");
        assert!(config.agent.enabled);
    }

    #[test]
    fn test_partial_config() {
        let toml = r#"
[server]
port = 3000
host = "localhost"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.memory.memory_dir, "~/.clawchorus/memory");
        assert_eq!(config.memory.chunk_size, 400);
        assert!(config.agent.enabled);
    }

    #[test]
    fn test_base_dir() {
        let dir = base_dir().unwrap();
        assert!(dir.ends_with(".clawchorus"));
    }

    #[test]
    fn test_config_path_ends_with_toml() {
        let path = config_path().unwrap();
        assert_eq!(path.file_name().unwrap(), "config.toml");
        assert!(path.parent().unwrap().ends_with(".clawchorus"));
    }

    #[test]
    fn test_memory_config_defaults() {
        let config = Config::default();
        assert_eq!(config.memory.memory_dir, "~/.clawchorus/memory");
        assert_eq!(config.memory.db_path, "~/.clawchorus/clawchorus.db");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
        assert_eq!(config.memory.temporal_decay_days, 30);
        assert_eq!(config.memory.hybrid_weight, 0.5);
    }

    #[test]
    fn test_memory_config_from_file() {
        let config = Config::from_file(test_config_path()).unwrap();
        assert_eq!(config.memory.memory_dir, "./test_store");
        assert_eq!(config.memory.db_path, ":memory:");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
    }
}
