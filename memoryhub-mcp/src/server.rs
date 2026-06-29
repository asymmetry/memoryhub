//! The MCP server: four tools forwarding to MemoryHub, plus agent identity wiring.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::client::{HttpClient, SearchResult};
use crate::config::Config;
use crate::error::{ClientError, UploadError};
use crate::identity;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// What to search your memory for.
    pub query: String,
    /// Search scope: "all" (default, whole team), "user", or "agent".
    #[serde(default)]
    pub scope: Option<String>,
    /// When true, exclude synthesized summaries and search raw memories only.
    #[serde(default)]
    pub raw_only: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteArgs {
    /// Optional project bucket to group this memory under (defaults to `_default`).
    #[serde(default)]
    pub project: Option<String>,
    /// The name to store this memory under (e.g. `decisions.md`). Re-using a name updates it.
    pub filename: String,
    /// The memory content to store.
    pub content: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UploadArgs {
    /// Optional project bucket to group this memory under (defaults to `_default`).
    #[serde(default)]
    pub project: Option<String>,
    /// The name to store this memory under (e.g. `decisions.md`). Re-using a name updates it.
    pub filename: String,
    /// Absolute path to a file on disk; its contents are read and stored under `filename`.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// The project bucket the memory was saved under (defaults to `_default` when omitted).
    #[serde(default)]
    pub project: Option<String>,
    /// The filename (name) of a memory previously saved by this agent.
    pub filename: String,
}

/// Renders search hits as readable lines; the full path reveals the originating agent.
pub fn format_search(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No matching memories.".to_string();
    }
    results
        .iter()
        .map(|r| format!("{} (score {:.2}):\n{}", r.path, r.score, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub async fn do_search(
    client: &HttpClient,
    agent_id: Uuid,
    scope: Option<&str>,
    raw_only: bool,
    query: &str,
) -> Result<String, ClientError> {
    Ok(format_search(
        &client.search(agent_id, scope, raw_only, query).await?,
    ))
}

pub async fn do_write(
    client: &HttpClient,
    agent_id: Uuid,
    project: Option<&str>,
    filename: &str,
    content: &str,
) -> Result<String, ClientError> {
    client.write(agent_id, project, filename, content).await?;
    Ok(format!("Saved memory '{}'.", filename))
}

pub async fn do_upload(
    client: &HttpClient,
    agent_id: Uuid,
    project: Option<&str>,
    filename: &str,
    path: &str,
) -> Result<String, UploadError> {
    if !Path::new(path).is_absolute() {
        return Err(UploadError::NotAbsolute);
    }
    let content =
        fs::read_to_string(path).map_err(|e| UploadError::Io(format!("{}: {}", path, e)))?;
    client.write(agent_id, project, filename, &content).await?;

    Ok(format!("Saved memory '{}'.", filename))
}

pub async fn do_read(
    client: &HttpClient,
    agent_id: Uuid,
    project: Option<&str>,
    filename: &str,
) -> Result<String, ClientError> {
    match client.read(agent_id, project, filename).await? {
        Some(content) => Ok(content),
        None => Ok(format!("No memory named '{}'.", filename)),
    }
}

#[derive(Clone)]
pub struct McpServer {
    client: HttpClient,
    config: Config,
    config_dir: PathBuf,
    agent_id: Arc<OnceCell<Uuid>>,
    #[allow(dead_code)]
    tool_router: ToolRouter<McpServer>,
}

#[tool_router]
impl McpServer {
    pub fn new(client: HttpClient, config: Config, config_dir: PathBuf) -> Self {
        Self {
            client,
            config,
            config_dir,
            agent_id: Arc::new(OnceCell::new()),
            tool_router: Self::tool_router(),
        }
    }

    /// Resolves the `agent_id` once, from the override or the connecting client's name.
    async fn agent_id(&self, ctx: &RequestContext<RoleServer>) -> Result<Uuid, McpError> {
        self.agent_id
            .get_or_try_init(|| async {
                let client_name = ctx
                    .peer
                    .peer_info()
                    .map(|info| info.client_info.name.clone());
                identity::resolve_agent_id(
                    self.config.agent_id,
                    client_name.as_deref(),
                    &self.config_dir,
                )
            })
            .await
            .copied()
            .map_err(|e| {
                McpError::internal_error(format!("could not resolve agent id: {}", e), None)
            })
    }

    #[tool(
        description = "Search your saved memory for relevant notes before starting a task. \
                       Returns matching memory files with snippets."
    )]
    async fn search_memory(
        &self,
        Parameters(args): Parameters<SearchArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.agent_id(&ctx).await?;
        match do_search(
            &self.client,
            agent_id,
            args.scope.as_deref(),
            args.raw_only,
            &args.query,
        )
        .await
        {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Save a memory you compose yourself. Provide the content directly, the \
                       filename to store it under, and an optional project bucket. Re-using a \
                       filename updates that memory."
    )]
    async fn write_memory(
        &self,
        Parameters(args): Parameters<WriteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.agent_id(&ctx).await?;
        match do_write(
            &self.client,
            agent_id,
            args.project.as_deref(),
            &args.filename,
            &args.content,
        )
        .await
        {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Save a memory file that already exists on disk. Pass its absolute path; \
                       the file is read and stored under the given filename and optional project \
                       bucket. Re-using a filename updates that memory."
    )]
    async fn upload_memory(
        &self,
        Parameters(args): Parameters<UploadArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.agent_id(&ctx).await?;
        match do_upload(
            &self.client,
            agent_id,
            args.project.as_deref(),
            &args.filename,
            &args.path,
        )
        .await
        {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Read the full content of a memory file you previously saved, by its \
                       filename and the project it was saved under."
    )]
    async fn read_memory(
        &self,
        Parameters(args): Parameters<ReadArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.agent_id(&ctx).await?;
        match do_read(
            &self.client,
            agent_id,
            args.project.as_deref(),
            &args.filename,
        )
        .await
        {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "MemoryHub gives you persistent memory across sessions. Call search_memory at the \
             start of a task to recall relevant context. Use write_memory to record durable \
             decisions, preferences, and facts you compose as you learn them, or upload_memory \
             to store a memory file that already exists on disk."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, method, path},
    };

    use super::*;

    #[tokio::test]
    async fn do_write_sends_model_authored_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/write"))
            .and(body_partial_json(serde_json::json!({
                "project": "notes",
                "filename": "decisions.md",
                "content": "we chose acktor",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let client = HttpClient::new(server.uri(), "mh_tok".into());
        let msg = do_write(
            &client,
            Uuid::new_v4(),
            Some("notes"),
            "decisions.md",
            "we chose acktor",
        )
        .await
        .unwrap();
        assert_eq!(msg, "Saved memory 'decisions.md'.");
    }

    #[test]
    fn format_search_empty_and_nonempty() {
        assert_eq!(format_search(&[]), "No matching memories.");
        let one = vec![SearchResult {
            path: "alice/abc/n.md".into(),
            start_line: 1,
            end_line: 2,
            score: 0.875,
            snippet: "snip".into(),
        }];
        let out = format_search(&one);
        assert!(out.contains("alice/abc/n.md"));
        assert!(out.contains("0.88"));
        assert!(out.contains("snip"));
    }

    #[tokio::test]
    async fn do_upload_reads_file_and_stores_under_given_filename() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/write"))
            .and(body_partial_json(serde_json::json!({
                "project": "notes",
                "filename": "ccc.md",
                "content": "remember this",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("local.md");
        std::fs::write(&file, "remember this").unwrap();
        let abs = file.to_str().unwrap();

        let client = HttpClient::new(server.uri(), "mh_tok".into());
        let msg = do_upload(&client, Uuid::new_v4(), Some("notes"), "ccc.md", abs)
            .await
            .unwrap();
        assert_eq!(msg, "Saved memory 'ccc.md'.");
    }

    #[tokio::test]
    async fn do_upload_rejects_relative_path() {
        let client = HttpClient::new("http://127.0.0.1:1".into(), "t".into());
        let err = do_upload(&client, Uuid::new_v4(), None, "x.md", "relative/x.md")
            .await
            .unwrap_err();
        assert!(matches!(err, UploadError::NotAbsolute));
    }

    #[tokio::test]
    async fn do_read_missing_is_friendly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/read"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"not_found"}"#))
            .mount(&server)
            .await;
        let client = HttpClient::new(server.uri(), "mh_tok".into());
        let msg = do_read(&client, Uuid::new_v4(), Some("notes"), "missing.md")
            .await
            .unwrap();
        assert_eq!(msg, "No memory named 'missing.md'.");
    }
}
