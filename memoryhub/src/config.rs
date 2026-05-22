use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
pub use crate::http::ServerConfig;
pub use crate::llm::LlmConfig;
pub use crate::memory::MemoryConfig;

/// Full application configuration loaded from `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub llm: LlmConfig,
}

/// Returns the base directory for MemoryHub data: `~/.memoryhub`.
pub fn base_dir() -> Result<PathBuf, ConfigError> {
    let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
    Ok(home.join(".memoryhub"))
}

/// Returns the default config file path: `~/.memoryhub/config.toml`.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(base_dir()?.join("config.toml"))
}

impl Config {
    /// Loads configuration from a TOML file at `path`.
    pub async fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ConfigError::Read {
                path: path.to_path_buf(),
                source: e,
            })?;
        let config: Config = toml::from_str(&raw).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            source: e.into(),
        })?;

        Ok(config)
    }

    /// Loads configuration from `path`, or from the default `~/.memoryhub/config.toml`
    /// when `path` is `None`.
    ///
    /// An explicit `path` that does not exist is an error. The default path is allowed
    /// to be missing, in which case built-in defaults are used (with a warning).
    pub async fn load(path: Option<PathBuf>) -> Result<Self, ConfigError> {
        let mut config = match path {
            Some(path) => {
                if !path.exists() {
                    return Err(ConfigError::Missing { path });
                }
                Self::from_file(path).await?
            }
            None => {
                let path = config_path()?;
                if path.exists() {
                    Self::from_file(path).await?
                } else {
                    tracing::warn!("~/.memoryhub/config.toml not found — using built-in defaults");
                    Self::default()
                }
            }
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

    #[tokio::test]
    async fn test_load_from_file() {
        let config = Config::from_file(test_config_path()).await.unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.memory.memory_dir, "./test_store");
        assert_eq!(config.memory.db_path, ":memory:");
    }

    #[tokio::test]
    async fn test_load_from_missing_file() {
        let result = Config::from_file("nonexistent.toml").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_explicit_missing_path_errors() {
        let result = Config::load(Some(PathBuf::from("definitely-not-here.toml"))).await;
        assert!(matches!(result, Err(ConfigError::Missing { .. })));
    }

    #[tokio::test]
    async fn test_load_explicit_path() {
        let config = Config::load(Some(test_config_path())).await.unwrap();
        assert_eq!(config.server.port, 9090);
    }

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.memory.memory_dir, "~/.memoryhub/memory");
        assert_eq!(config.memory.db_path, "~/.memoryhub/memoryhub.db");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
        assert_eq!(config.llm.provider, "deepseek");
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
        assert_eq!(config.memory.memory_dir, "~/.memoryhub/memory");
        assert_eq!(config.memory.chunk_size, 400);
    }

    #[test]
    fn test_base_dir() {
        let dir = base_dir().unwrap();
        assert!(dir.ends_with(".memoryhub"));
    }

    #[test]
    fn test_config_path_ends_with_toml() {
        let path = config_path().unwrap();
        assert_eq!(path.file_name().unwrap(), "config.toml");
        assert!(path.parent().unwrap().ends_with(".memoryhub"));
    }

    #[test]
    fn test_memory_config_defaults() {
        let config = Config::default();
        assert_eq!(config.memory.memory_dir, "~/.memoryhub/memory");
        assert_eq!(config.memory.db_path, "~/.memoryhub/memoryhub.db");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
        assert_eq!(config.memory.temporal_decay_days, 30);
        assert_eq!(config.memory.hybrid_weight, 0.5);
    }

    #[tokio::test]
    async fn test_memory_config_from_file() {
        let config = Config::from_file(test_config_path()).await.unwrap();
        assert_eq!(config.memory.memory_dir, "./test_store");
        assert_eq!(config.memory.db_path, ":memory:");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
    }
}
