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

/// Name of the environment variable holding the base directory override.
pub const BASE_DIR_ENV: &str = "MEMORYHUB_HOME";

/// Resolves the base directory for all MemoryHub data.
///
/// Precedence: the `override_dir` argument (the `--base-dir` flag) > the
/// `MEMORYHUB_HOME` environment variable > `~/.memoryhub`. A home directory is
/// only required for the final fallback, so a container can run without one by
/// setting `MEMORYHUB_HOME`.
pub fn base_dir(override_dir: Option<&Path>) -> Result<PathBuf, ConfigError> {
    if let Some(dir) = override_dir {
        return Ok(dir.to_path_buf());
    }
    if let Some(dir) = std::env::var_os(BASE_DIR_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
    Ok(home.join(".memoryhub"))
}

/// Returns the default config file path: `{base}/config.toml`.
pub fn config_path(base: &Path) -> PathBuf {
    base.join("config.toml")
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

    /// Loads configuration from `path`, or from the default `{base_dir}/config.toml`
    /// when `path` is `None`, then resolves all data paths against `base_dir`.
    ///
    /// An explicit `path` that does not exist is an error. The default path is allowed
    /// to be missing, in which case built-in defaults are used (with a warning).
    pub async fn load(path: Option<PathBuf>, base_dir: &Path) -> Result<Self, ConfigError> {
        let mut config = match path {
            Some(path) => {
                if !path.exists() {
                    return Err(ConfigError::Missing { path });
                }
                Self::from_file(path).await?
            }
            None => {
                let path = config_path(base_dir);
                if path.exists() {
                    Self::from_file(path).await?
                } else {
                    tracing::warn!(
                        "{} not found — using built-in defaults",
                        config_path(base_dir).display()
                    );
                    Self::default()
                }
            }
        };
        config.memory.resolve_paths(base_dir)?;
        config.llm.resolve_paths(base_dir);
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

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
        let result = Config::load(
            Some(PathBuf::from("definitely-not-here.toml")),
            Path::new("."),
        )
        .await;
        assert!(matches!(result, Err(ConfigError::Missing { .. })));
    }

    #[tokio::test]
    async fn test_load_explicit_path() {
        let config = Config::load(Some(test_config_path()), Path::new("."))
            .await
            .unwrap();
        assert_eq!(config.server.port, 9090);
    }

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.memory.memory_dir, "memory");
        assert_eq!(config.memory.db_path, "memoryhub.db");
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
        assert_eq!(config.memory.memory_dir, "memory");
        assert_eq!(config.memory.chunk_size, 400);
    }

    #[test]
    fn test_base_dir_flag_override_wins() {
        let dir = base_dir(Some(Path::new("/data/mh"))).unwrap();
        assert_eq!(dir, PathBuf::from("/data/mh"));
    }

    #[test]
    fn test_base_dir_default_ends_with_memoryhub() {
        // No flag override; without MEMORYHUB_HOME set this falls back to the home dir.
        if std::env::var_os(BASE_DIR_ENV).is_none() {
            let dir = base_dir(None).unwrap();
            assert!(dir.ends_with(".memoryhub"));
        }
    }

    #[test]
    fn test_config_path_joins_base() {
        let path = config_path(Path::new("/data/mh"));
        assert_eq!(path, PathBuf::from("/data/mh").join("config.toml"));
    }

    #[test]
    fn test_memory_config_defaults() {
        let config = Config::default();
        assert_eq!(config.memory.memory_dir, "memory");
        assert_eq!(config.memory.db_path, "memoryhub.db");
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
