use serde::{Deserialize, Serialize};

/// LLM / embedding provider settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider identifier, e.g. `"deepseek"`.
    pub provider: String,
    /// Name of the environment variable that holds the API key.
    pub api_key_env: String,
    /// Chat / completion model identifier.
    pub model: String,
    /// Embedding model identifier.
    pub embedding_model: String,
    /// Embedding vector dimension. `None` means auto-detect from the first
    /// embedding response. Pin it for known models to avoid surprises.
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    /// Seconds a `Session` will sit idle before self-terminating.
    #[serde(default = "default_session_idle_timeout_secs")]
    pub session_idle_timeout_secs: u64,
    /// Maximum number of attempts (including the first) for a single
    /// provider call on transient failure.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// HTTP request timeout in seconds for provider calls.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Provider base URL (override for testing / self-hosted gateways).
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

const fn default_session_idle_timeout_secs() -> u64 {
    600
}

const fn default_max_retries() -> u32 {
    3
}

const fn default_request_timeout_secs() -> u64 {
    30
}

fn default_base_url() -> String {
    "https://api.deepseek.com".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            model: "deepseek-chat".to_string(),
            embedding_model: "deepseek-embedding".to_string(),
            embedding_dim: None,
            session_idle_timeout_secs: 600,
            max_retries: 3,
            request_timeout_secs: 30,
            base_url: "https://api.deepseek.com".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_populated() {
        let c = LlmConfig::default();
        assert_eq!(c.session_idle_timeout_secs, 600);
        assert_eq!(c.max_retries, 3);
        assert_eq!(c.request_timeout_secs, 30);
        assert_eq!(c.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn partial_toml_uses_defaults_for_new_fields() {
        let toml_in = r#"
provider = "deepseek"
api_key_env = "DEEPSEEK_API_KEY"
model = "deepseek-chat"
embedding_model = "deepseek-embedding"
"#;
        let c: LlmConfig = toml::from_str(toml_in).unwrap();
        assert_eq!(c.max_retries, 3);
        assert_eq!(c.request_timeout_secs, 30);
        assert_eq!(c.base_url, "https://api.deepseek.com");
    }
}
