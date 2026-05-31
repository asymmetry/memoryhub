//! The MCP server: three tools forwarding to MemoryHub, plus agent identity wiring.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, model::*, schemars, tool, tool_handler,
    tool_router,
};
use serde::Deserialize;
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::client::{ClientError, MemoryClient, SearchResult};
use crate::config::Config;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// What to search your memory for.
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveArgs {
    /// Absolute path to a memory file the agent has written. The absolute path is used as the
    /// stored filename; the model does not name the memory.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// The absolute path (filename) of a memory previously saved by this agent.
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

/// Errors specific to `save_memory`'s filesystem handling.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("path must be absolute")]
    NotAbsolute,

    #[error("cannot read {0}")]
    Io(String),

    #[error(transparent)]
    Client(#[from] ClientError),
}

// --- Tool cores (network + formatting; unit-tested directly) ---

pub async fn do_search(
    client: &MemoryClient,
    agent_id: Uuid,
    query: &str,
) -> Result<String, ClientError> {
    Ok(format_search(&client.search(agent_id, query).await?))
}

pub async fn do_save(
    client: &MemoryClient,
    agent_id: Uuid,
    path: &str,
) -> Result<String, SaveError> {
    if !std::path::Path::new(path).is_absolute() {
        return Err(SaveError::NotAbsolute);
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| SaveError::Io(format!("{}: {}", path, e)))?;
    client.write(agent_id, path, &content).await?;
    Ok(format!("Saved memory '{}'.", path))
}

pub async fn do_read(
    client: &MemoryClient,
    agent_id: Uuid,
    filename: &str,
) -> Result<String, ClientError> {
    match client.read(agent_id, filename).await? {
        Some(content) => Ok(content),
        None => Ok(format!("No memory named '{}'.", filename)),
    }
}

// --- rmcp server ---

#[derive(Clone)]
pub struct MemoryServer {
    client: MemoryClient,
    config: Config,
    config_dir: PathBuf,
    agent_id: Arc<OnceCell<Uuid>>,
    // Read by the `#[tool_handler]`-generated dispatch, which rustc's dead-code analysis can't see.
    #[allow(dead_code)]
    tool_router: ToolRouter<MemoryServer>,
}

#[tool_router]
impl MemoryServer {
    pub fn new(client: MemoryClient, config: Config, config_dir: PathBuf) -> Self {
        Self {
            client,
            config,
            config_dir,
            agent_id: Arc::new(OnceCell::new()),
            tool_router: Self::tool_router(),
        }
    }

    /// Resolves the `agent_id` once, from the override or the connecting client's name.
    async fn agent_id(&self, ctx: &RequestContext<RoleServer>) -> Uuid {
        *self
            .agent_id
            .get_or_init(|| async {
                let client_name = ctx
                    .peer
                    .peer_info()
                    .map(|info| info.client_info.name.clone());
                crate::identity::resolve_agent_id(
                    self.config.agent_id_override,
                    client_name.as_deref(),
                    &self.config_dir,
                )
                .unwrap_or_else(|_| Uuid::nil())
            })
            .await
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
        let agent_id = self.agent_id(&ctx).await;
        match do_search(&self.client, agent_id, &args.query).await {
            Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Persist a memory file you have written to disk. Pass its absolute path; \
                       the file is read and stored under that absolute path as its name. \
                       Re-saving the same path updates it."
    )]
    async fn save_memory(
        &self,
        Parameters(args): Parameters<SaveArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.agent_id(&ctx).await;
        match do_save(&self.client, agent_id, &args.path).await {
            Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Read the full content of a memory file you previously saved, by its \
                       absolute path."
    )]
    async fn read_memory(
        &self,
        Parameters(args): Parameters<ReadArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.agent_id(&ctx).await;
        match do_read(&self.client, agent_id, &args.filename).await {
            Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "MemoryHub gives you persistent memory across sessions. Call search_memory at the \
             start of a task to recall relevant context, and save_memory to record durable \
             decisions, preferences, and facts as you learn them."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
    async fn do_save_reads_file_and_uses_absolute_path_as_filename() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/write"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ccc.md");
        std::fs::write(&file, "remember this").unwrap();
        let abs = file.to_str().unwrap();

        let client = MemoryClient::new(server.uri(), "mh_tok".into());
        let msg = do_save(&client, Uuid::new_v4(), abs).await.unwrap();
        assert_eq!(msg, format!("Saved memory '{}'.", abs));
    }

    #[tokio::test]
    async fn do_save_rejects_relative_path() {
        let client = MemoryClient::new("http://127.0.0.1:1".into(), "t".into());
        let err = do_save(&client, Uuid::new_v4(), "relative/x.md")
            .await
            .unwrap_err();
        assert!(matches!(err, SaveError::NotAbsolute));
    }

    #[tokio::test]
    async fn do_read_missing_is_friendly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/read"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"not_found"}"#))
            .mount(&server)
            .await;
        let client = MemoryClient::new(server.uri(), "mh_tok".into());
        let msg = do_read(&client, Uuid::new_v4(), "missing.md")
            .await
            .unwrap();
        assert_eq!(msg, "No memory named 'missing.md'.");
    }
}
