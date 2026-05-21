//! Prompt template resolution and loading for synthesis.
//!
//! On startup [`write_default_templates`] writes the defaults into `prompts_dir` if they do not
//! already exist, so they can be edited on disk. [`load_template`] reads from disk first, and
//! falls back to the default if no on-disk file exists.

use std::path::Path;

use tokio::fs;

use super::error::LlmError;

const DEFAULT_PER_USER: &str = include_str!("prompts/per_user.md");
const DEFAULT_GLOBAL: &str = include_str!("prompts/global.md");

/// Which synthesis prompt a task uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    PerUser,
    Global,
}

impl TemplateKind {
    fn filename(self) -> &'static str {
        match self {
            TemplateKind::PerUser => "per_user.md",
            TemplateKind::Global => "global.md",
        }
    }

    /// Default template content compiled into the binary.
    pub fn default_content(self) -> &'static str {
        match self {
            TemplateKind::PerUser => DEFAULT_PER_USER,
            TemplateKind::Global => DEFAULT_GLOBAL,
        }
    }
}

/// Loads the synthesis prompt template for `(provider_name, kind)`.
///
/// Resolution order:
/// 1. `{prompts_dir}/{provider_name}/{kind}.md` if it exists,
/// 2. else `{prompts_dir}/{kind}.md` if it exists,
/// 3. else the embedded default ([`TemplateKind::default_content`]).
///
/// On-disk files are read fresh on every call so edits take effect without a restart.
pub async fn load_template(
    prompts_dir: &Path,
    provider_name: &str,
    kind: TemplateKind,
) -> Result<String, LlmError> {
    let specific = prompts_dir.join(provider_name).join(kind.filename());
    if specific.is_file() {
        return fs::read_to_string(&specific).await.map_err(Into::into);
    }

    let default_path = prompts_dir.join(kind.filename());
    if default_path.is_file() {
        return fs::read_to_string(&default_path).await.map_err(Into::into);
    }

    Ok(kind.default_content().to_string())
}

/// Writes default prompt templates to `prompts_dir` if they do not already exist.
///
/// Creates `prompts_dir` if necessary and writes the embedded default for every [`TemplateKind`]
/// whose default file is not yet present.
pub async fn write_default_templates(prompts_dir: &Path) -> Result<(), LlmError> {
    fs::create_dir_all(prompts_dir)
        .await
        .map_err(LlmError::WriteDefaultTemplates)?;
    for kind in [TemplateKind::PerUser, TemplateKind::Global] {
        let path = prompts_dir.join(kind.filename());
        if !path.exists() {
            fs::write(&path, kind.default_content())
                .await
                .map_err(LlmError::WriteDefaultTemplates)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn provider_specific_template_wins_over_default() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "per_user.md", "default body");
        write(dir.path(), "deepseek/per_user.md", "deepseek body");

        let out = load_template(dir.path(), "deepseek", TemplateKind::PerUser)
            .await
            .unwrap();
        assert_eq!(out, "deepseek body");
    }

    #[tokio::test]
    async fn on_disk_default_wins_over_embedded() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "global.md", "on-disk global");

        let out = load_template(dir.path(), "deepseek", TemplateKind::Global)
            .await
            .unwrap();
        assert_eq!(out, "on-disk global");
    }

    #[tokio::test]
    async fn missing_template_falls_back_to_embedded_default() {
        let dir = tempfile::tempdir().unwrap();
        let out = load_template(dir.path(), "deepseek", TemplateKind::PerUser)
            .await
            .unwrap();
        assert_eq!(out, DEFAULT_PER_USER);
    }

    #[tokio::test]
    async fn ensure_default_templates_writes_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        write_default_templates(dir.path()).await.unwrap();
        let per_user = std::fs::read_to_string(dir.path().join("per_user.md")).unwrap();
        let global = std::fs::read_to_string(dir.path().join("global.md")).unwrap();
        assert_eq!(per_user, DEFAULT_PER_USER);
        assert_eq!(global, DEFAULT_GLOBAL);
    }

    #[tokio::test]
    async fn ensure_default_templates_preserves_existing_edits() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "per_user.md", "edited");
        write_default_templates(dir.path()).await.unwrap();
        let per_user = std::fs::read_to_string(dir.path().join("per_user.md")).unwrap();
        assert_eq!(per_user, "edited");
    }
}
