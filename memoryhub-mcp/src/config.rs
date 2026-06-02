//! Runtime configuration loaded from environment variables.

use uuid::Uuid;

use crate::error::ConfigError;

/// Validated runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub url: String,
    pub token: String,
    pub agent_id: Option<Uuid>,
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
}
