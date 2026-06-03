//! Hook-CLI subcommand cores. Pure-ish: I/O wiring lives in `main.rs`.

use std::io::Read;

use serde::Deserialize;
use uuid::Uuid;

use crate::client::MemoryHubClient;
use crate::server::do_upload;

/// One file to upload, as sent by a plugin hook on stdin.
#[derive(Debug, Deserialize)]
pub struct UploadItem {
    #[serde(default)]
    pub project: Option<String>,
    pub filename: String,
    pub path: String,
}

/// Parse a JSON array of upload items from a reader.
///
/// I/O errors are propagated as `Err`. JSON parse errors are silently treated
/// as an empty list — a hook must never crash the session because of bad stdin.
pub fn parse_items<R: Read>(mut reader: R) -> std::io::Result<Vec<UploadItem>> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    Ok(serde_json::from_str(&buf).unwrap_or_default())
}

/// Upload each item; returns a `(filename, error)` list of failures (never panics).
pub async fn upload_items(
    client: &MemoryHubClient,
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

    use super::*;
    use crate::client::MemoryHubClient;

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
        let client = MemoryHubClient::new(server.uri(), "t".into());
        let failures = upload_items(&client, Uuid::new_v4(), items).await;
        assert!(failures.is_empty(), "failures: {failures:?}");
    }
}
