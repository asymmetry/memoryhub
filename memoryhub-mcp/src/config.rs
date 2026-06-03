//! Runtime configuration loaded from environment variables.

use std::path::Path;
use uuid::Uuid;

use crate::error::ConfigError;

/// Validated runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub url: String,
    pub token: String,
    pub agent_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct FileConfig {
    url: Option<String>,
    token: Option<String>,
}

impl Config {
    /// Builds a `Config` from already-read values. Empty strings are treated as absent.
    pub fn load(
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
            url: url.trim_end_matches('/').to_string(),
            token,
            agent_id,
        })
    }

    /// Reads the standard environment variables.
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::load(
            std::env::var("MEMORYHUB_URL").ok(),
            std::env::var("MEMORYHUB_TOKEN").ok(),
            std::env::var("MEMORYHUB_AGENT_ID").ok(),
        )
    }

    /// Resolve `(url, token)` for hook-CLI mode: env first, then
    /// `<config_dir>/config.json`. `config_dir` is the `…/memoryhub` dir.
    pub fn load_connection(
        config_dir: &Path,
        env_url: Option<String>,
        env_token: Option<String>,
    ) -> Result<(String, String), ConfigError> {
        let file: FileConfig = std::fs::read_to_string(config_dir.join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let url = env_url
            .filter(|s| !s.is_empty())
            .or(file.url)
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::MissingUrl)?;
        let token = env_token
            .filter(|s| !s.is_empty())
            .or(file.token)
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::MissingToken)?;
        Ok((url.trim_end_matches('/').to_string(), token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_requires_url_and_token() {
        assert_eq!(
            Config::load(None, Some("t".into()), None).unwrap_err(),
            ConfigError::MissingUrl
        );
        assert_eq!(
            Config::load(Some("u".into()), Some(String::new()), None).unwrap_err(),
            ConfigError::MissingToken
        );
    }

    #[test]
    fn load_trims_trailing_slash_and_parses_override() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let cfg = Config::load(
            Some("http://x:8000/".into()),
            Some("mh_tok".into()),
            Some(id.into()),
        )
        .unwrap();
        assert_eq!(cfg.url, "http://x:8000");
        assert_eq!(cfg.token, "mh_tok");
        assert_eq!(cfg.agent_id.unwrap().to_string(), id);
    }

    #[test]
    fn load_rejects_bad_override_uuid() {
        let err = Config::load(
            Some("u".into()),
            Some("t".into()),
            Some("not-a-uuid".into()),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::BadAgentId(_)));
    }

    #[test]
    fn load_no_override_is_none() {
        let cfg = Config::load(Some("u".into()), Some("t".into()), None).unwrap();
        assert!(cfg.agent_id.is_none());
    }

    #[test]
    fn load_connection_prefers_env_then_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"url":"http://file:8000/","token":"file_tok"}"#,
        )
        .unwrap();

        // File fallback when env absent.
        let (url, token) = Config::load_connection(dir.path(), None, None).unwrap();
        assert_eq!(url, "http://file:8000");
        assert_eq!(token, "file_tok");

        // Env wins.
        let (url, token) = Config::load_connection(
            dir.path(),
            Some("http://env:9/".into()),
            Some("env_tok".into()),
        )
        .unwrap();
        assert_eq!(url, "http://env:9");
        assert_eq!(token, "env_tok");
    }

    #[test]
    fn load_connection_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Config::load_connection(dir.path(), None, None).is_err());
    }
}
