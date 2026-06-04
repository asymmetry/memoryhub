//! Hook-CLI argument parsing and subcommand cores. Pure-ish: I/O wiring lives in `main.rs`.

use std::io::{Read, Result};

use clap::{Parser, Subcommand};
use serde::Deserialize;
use uuid::Uuid;

use memoryhub_mcp::{client::HttpClient, server::do_upload};

#[derive(Parser)]
#[command(name = "memoryhub-mcp")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Write url/token to <config_dir>/memoryhub/config.toml (interactive),
    /// or with --check, exit 0 if configured and non-zero otherwise (no prompts).
    Config {
        /// Exit 0 if url+token resolve (env or config.toml), non-zero otherwise. No prompts.
        #[arg(long)]
        check: bool,
    },
    /// Upload memory files (stdin JSON array, or a single --filename/--path).
    Upload {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        filename: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
    /// Print the latest synthesized summary for the agent's scope.
    Recall {
        #[arg(long)]
        agent: String,
        #[arg(long, default_value = "user")]
        scope: String,
    },
}

/// One file to upload, as sent by a plugin hook on stdin.
#[derive(Debug, Deserialize)]
pub struct UploadItem {
    #[serde(default)]
    pub project: Option<String>,
    pub filename: String,
    pub path: String,
}

/// Parses a JSON array of upload items from a reader.
///
/// I/O errors are propagated as `Err`. JSON parse errors are silently treated as an empty list
/// since a hook must never crash the session because of bad stdin.
pub fn parse_items<R>(mut reader: R) -> Result<Vec<UploadItem>>
where
    R: Read,
{
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;

    Ok(serde_json::from_str(&buf).unwrap_or_default())
}

/// Uploads each item.
///
/// Returns a `(filename, error)` list of failures (never panics).
pub async fn upload_items(
    client: &HttpClient,
    agent_id: Uuid,
    items: Vec<UploadItem>,
) -> Vec<(String, String)> {
    let mut failures = Vec::new();
    for item in items {
        if let Err(e) = do_upload(
            client,
            agent_id,
            item.project.as_deref(),
            &item.filename,
            &item.path,
        )
        .await
        {
            failures.push((item.filename, e.to_string()));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use memoryhub_mcp::client::HttpClient;

    use super::*;

    #[test]
    fn parse_upload_items_from_json() {
        let json = r#"[{"project":"p","filename":"a.md","path":"/x/a.md"},
                       {"filename":"b.md","path":"/x/b.md"}]"#;
        let items = parse_items(json.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].project.as_deref(), Some("p"));
        assert!(items[1].project.is_none());
    }

    #[test]
    fn parse_items_swallows_bad_json() {
        let items = parse_items(b"not json" as &[u8]).unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn upload_items_posts_each_file() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/write"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(2)
            .mount(&server)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let mut items = Vec::new();
        for name in ["a.md", "b.md"] {
            let p = tmp.path().join(name);
            std::fs::write(&p, "x").unwrap();
            items.push(UploadItem {
                project: None,
                filename: name.to_string(),
                path: p.to_string_lossy().to_string(),
            });
        }
        let client = HttpClient::new(server.uri(), "t".into());
        let failures = upload_items(&client, Uuid::new_v4(), items).await;
        assert!(failures.is_empty(), "failures: {failures:?}");
    }
}
