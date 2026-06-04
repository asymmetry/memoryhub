//! Runtime configuration loaded from environment variables.

use std::env;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ConfigError;

/// Validated runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub url: String,
    pub token: String,
    pub agent_id: Option<Uuid>,
}

/// On-disk shape of `config.toml`: the hook-CLI credentials.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FileConfig {
    pub url: Option<String>,
    pub token: Option<String>,
}

impl Config {
    /// Builds a `Config` from values. Empty strings are treated as absent.
    pub fn new(
        url: Option<String>,
        token: Option<String>,
        agent_id: Option<String>,
    ) -> Result<Config, ConfigError> {
        let url = url
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::MissingUrl)?;
        let token = token
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::MissingToken)?;
        let agent_id = match agent_id.filter(|s| !s.is_empty()) {
            Some(s) => Some(s.parse::<Uuid>().map_err(|_| ConfigError::BadAgentId(s))?),
            None => None,
        };

        Ok(Config {
            url,
            token,
            agent_id,
        })
    }

    /// Builds a `Config` from the standard environment variables.
    pub fn from_envs() -> Result<Config, ConfigError> {
        Config::new(
            env::var("MEMORYHUB_URL").ok(),
            env::var("MEMORYHUB_TOKEN").ok(),
            env::var("MEMORYHUB_AGENT_ID").ok(),
        )
    }

    /// Builds a `Config` from `config_file`, then lets the standard `MEMORYHUB_*` env vars
    /// override the values.
    pub fn from_file(config_file: &Path) -> Result<Config, ConfigError> {
        let file: FileConfig = fs::read_to_string(config_file)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();

        let env = |key: &str| env::var(key).ok().filter(|s| !s.is_empty());

        Config::new(
            env("MEMORYHUB_URL").or(file.url),
            env("MEMORYHUB_TOKEN").or(file.token),
            env("MEMORYHUB_AGENT_ID"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_requires_url_and_token() {
        assert_eq!(
            Config::new(None, Some("t".into()), None).unwrap_err(),
            ConfigError::MissingUrl
        );
        assert_eq!(
            Config::new(Some("u".into()), Some(String::new()), None).unwrap_err(),
            ConfigError::MissingToken
        );
    }

    #[test]
    fn load_passes_url_through_and_parses_override() {
        // URL normalization lives in `HttpClient::new`, so `load` keeps the URL as-is.
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let cfg = Config::new(
            Some("http://x:8000/".into()),
            Some("mh_tok".into()),
            Some(id.into()),
        )
        .unwrap();
        assert_eq!(cfg.url, "http://x:8000/");
        assert_eq!(cfg.token, "mh_tok");
        assert_eq!(cfg.agent_id.unwrap().to_string(), id);
    }

    #[test]
    fn load_rejects_bad_override_uuid() {
        let err = Config::new(
            Some("u".into()),
            Some("t".into()),
            Some("not-a-uuid".into()),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::BadAgentId(_)));
    }

    #[test]
    fn load_no_override_is_none() {
        let cfg = Config::new(Some("u".into()), Some("t".into()), None).unwrap();
        assert!(cfg.agent_id.is_none());
    }

    #[test]
    fn from_file_reads_file_when_env_absent() {
        // Assumes the MEMORYHUB_* env vars are not set in the test environment.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "url = \"http://file:8000/\"\ntoken = \"file_tok\"\n").unwrap();

        let cfg = Config::from_file(&file).unwrap();
        // URL passed through; normalization is the client's job.
        assert_eq!(cfg.url, "http://file:8000/");
        assert_eq!(cfg.token, "file_tok");
        assert!(cfg.agent_id.is_none());
    }

    #[test]
    fn from_file_errors_when_missing() {
        // Assumes the MEMORYHUB_* env vars are not set in the test environment.
        let dir = tempfile::tempdir().unwrap();
        assert!(Config::from_file(&dir.path().join("config.toml")).is_err());
    }
}
