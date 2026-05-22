use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::warn;

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
const BASE_DIR_ENV: &str = "MEMORYHUB_HOME";

impl Config {
    /// Resolves the base directory for all MemoryHub data.
    ///
    /// Precedence: the `custom_base_dir` argument (the `--base-dir` flag) > the `MEMORYHUB_HOME`
    /// environment variable > `~/.memoryhub`.
    pub fn base_dir(custom_base_dir: Option<&Path>) -> Result<PathBuf, ConfigError> {
        if let Some(dir) = custom_base_dir {
            return Ok(dir.to_path_buf());
        }

        if let Some(dir) = std::env::var_os(BASE_DIR_ENV).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(dir));
        }

        let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;

        Ok(home.join(".memoryhub"))
    }

    /// Loads configuration from a TOML file at `path`.
    pub async fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
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

    /// Loads configuration from `path`, or from the default `{base_dir}/config.toml` when `path`
    /// is `None`, then resolves all data paths against `base_dir`.
    ///
    /// An explicit `path` that does not exist is an error. The default path is allowed to be
    /// missing, in which case built-in defaults are used (with a warning).
    pub async fn load(path: Option<PathBuf>, base_dir: &Path) -> Result<Self, ConfigError> {
        let mut config = match path {
            Some(path) => {
                if !path.exists() {
                    return Err(ConfigError::Missing { path });
                }

                Self::from_file(path).await?
            }
            None => {
                let path = base_dir.join("config.toml");
                if path.exists() {
                    Self::from_file(path).await?
                } else {
                    warn!("{} not found — using built-in defaults", path.display());

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

    #[test]
    fn defaults() {
        // Built-in defaults.
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.memory.memory_dir, "memory");
        assert_eq!(config.memory.db_path, "memoryhub.db");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.memory.chunk_overlap, 80);
        assert_eq!(config.llm.provider, "deepseek");

        // A partial TOML overrides only what it specifies; the rest fall back to defaults.
        let toml = r#"
[server]
port = 3000
host = "localhost"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.host, "localhost");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.memory.memory_dir, "memory");
        assert_eq!(config.memory.chunk_size, 400);
        assert_eq!(config.llm.provider, "deepseek");
    }

    #[tokio::test]
    async fn load() {
        let config = Config::load(Some(test_config_path()), Path::new("."))
            .await
            .unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
    }

    #[tokio::test]
    async fn load_from_missing_file() {
        let result = Config::load(
            Some(PathBuf::from("definitely-not-here.toml")),
            Path::new("."),
        )
        .await;
        assert!(matches!(result, Err(ConfigError::Missing { .. })));
    }

    #[test]
    fn base_dir_flag_override_wins() {
        let dir = Config::base_dir(Some(Path::new("/data/mh"))).unwrap();
        assert_eq!(dir, PathBuf::from("/data/mh"));
    }

    #[test]
    fn base_dir_default_ends_with_memoryhub() {
        if std::env::var_os(BASE_DIR_ENV).is_none() {
            let dir = Config::base_dir(None).unwrap();
            assert!(dir.ends_with(".memoryhub"));
        }
    }
}
