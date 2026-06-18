use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// LLM / embedding provider settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Chat provider identifier.
    pub provider: String,
    /// Embedding provider identifier. Kept separate from `provider` because not every chat vendor
    /// exposes an embeddings endpoint.
    pub embedding_provider: String,
    /// Name of the environment variable that holds the chat API key.
    pub api_key_env: String,
    /// Name of the environment variable that holds the embedding API key.
    pub embedding_api_key_env: String,
    /// Chat / completion model identifier.
    pub model: String,
    /// Embedding model identifier.
    pub embedding_model: String,
    /// Embedding vector dimension.
    ///
    /// `None` means auto-detect from the first embedding response. When set, it is both enforced
    /// on every response and sent to the provider as the `dimensions` request parameter, so the
    /// returned vectors actually match.
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    /// Directory holding synthesis prompt templates.
    #[serde(default = "default_prompts_dir")]
    pub prompts_dir: PathBuf,
    /// Seconds a `SynthesisTask` will sit idle before self-terminating.
    #[serde(default = "default_synthesis_idle_timeout_secs")]
    pub synthesis_idle_timeout_secs: u64,
    /// Character budget for a `SynthesisTask`'s conversation history; once
    /// exceeded, the history is cleared and reseeded from the prior summary.
    #[serde(default = "default_synthesis_context_max_chars")]
    pub synthesis_context_max_chars: usize,
    /// Maximum number of attempts (including the first) for a single
    /// provider call on transient failure.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// HTTP request timeout in seconds for provider calls.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Chat provider base URL (override for testing / self-hosted gateways).
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Embedding provider base URL (override for testing / self-hosted gateways).
    #[serde(default = "default_embedding_base_url")]
    pub embedding_base_url: String,
}

/// Default prompts directory, relative to the base directory: `prompts`.
fn default_prompts_dir() -> PathBuf {
    PathBuf::from("prompts")
}

const fn default_synthesis_idle_timeout_secs() -> u64 {
    300
}

const fn default_synthesis_context_max_chars() -> usize {
    200_000
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

fn default_embedding_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

impl LlmConfig {
    /// Resolves `prompts_dir` against the base directory: an absolute path is
    /// left unchanged, a relative one is joined onto `base`.
    pub fn resolve_paths(&mut self, base: &Path) {
        if self.prompts_dir.is_relative() {
            self.prompts_dir = base.join(&self.prompts_dir);
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".to_string(),
            embedding_provider: "openai".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            embedding_api_key_env: "OPENAI_API_KEY".to_string(),
            model: "deepseek-v4-flash".to_string(),
            embedding_model: "text-embedding-3-small".to_string(),
            embedding_dim: None,
            synthesis_idle_timeout_secs: default_synthesis_idle_timeout_secs(),
            prompts_dir: default_prompts_dir(),
            synthesis_context_max_chars: default_synthesis_context_max_chars(),
            max_retries: default_max_retries(),
            request_timeout_secs: default_request_timeout_secs(),
            base_url: default_base_url(),
            embedding_base_url: default_embedding_base_url(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = LlmConfig::default();
        assert_eq!(
            c.synthesis_idle_timeout_secs,
            default_synthesis_idle_timeout_secs()
        );
        assert_eq!(c.max_retries, default_max_retries());
        assert_eq!(c.request_timeout_secs, default_request_timeout_secs());
        assert_eq!(c.base_url, default_base_url());
        assert_eq!(c.prompts_dir, PathBuf::from("prompts"));
        assert_eq!(
            c.synthesis_context_max_chars,
            default_synthesis_context_max_chars()
        );

        let toml_in = r#"
provider = "deepseek"
embedding_provider = "openai"
api_key_env = "DEEPSEEK_API_KEY"
embedding_api_key_env = "OPENAI_API_KEY"
model = "deepseek-chat"
embedding_model = "text-embedding-3-small"
"#;
        let c: LlmConfig = toml::from_str(toml_in).unwrap();
        assert_eq!(c.max_retries, default_max_retries());
        assert_eq!(
            c.synthesis_idle_timeout_secs,
            default_synthesis_idle_timeout_secs()
        );
        assert_eq!(c.prompts_dir, PathBuf::from("prompts"));
        assert_eq!(
            c.synthesis_context_max_chars,
            default_synthesis_context_max_chars()
        );
    }

    #[test]
    fn resolve_paths_joins_relative_paths_onto_base() {
        let base = Path::new("/data/mh");
        let mut c = LlmConfig::default();
        c.resolve_paths(base);
        assert_eq!(c.prompts_dir, base.join("prompts"));
    }

    #[test]
    fn resolve_paths_leaves_absolute_paths_unchanged() {
        let abs = if cfg!(windows) {
            PathBuf::from("C:\\custom\\prompts")
        } else {
            PathBuf::from("/custom/prompts")
        };
        let mut c = LlmConfig {
            prompts_dir: abs.clone(),
            ..LlmConfig::default()
        };
        c.resolve_paths(Path::new("/data/mh"));
        assert_eq!(c.prompts_dir, abs);
    }
}
