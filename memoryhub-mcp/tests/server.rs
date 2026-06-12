use std::path::PathBuf;

use rmcp::{ServiceExt, model::CallToolRequestParams};
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use memoryhub_mcp::{client::HttpClient, config::Config, server::McpServer};

#[tokio::test]
async fn client_lists_and_calls_tools() {
    // Backend the server's HTTP client will talk to.
    let backend = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/memories/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [{
                "path": "alice/abc/notes.md",
                "start_line": 1, "end_line": 2, "score": 0.9, "snippet": "hello world"
            }]
        })))
        .mount(&backend)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/memories/write"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&backend)
        .await;

    // Pin agent_id via the override so resolution needs no client name or disk.
    let config = Config {
        url: backend.uri(),
        token: "mh_tok".into(),
        agent_id: Some(Uuid::new_v4()),
    };
    let client = HttpClient::new(config.url.clone(), config.token.clone());
    let server = McpServer::new(client, config, PathBuf::from("/nonexistent"));

    // In-memory transport: serve the server on one end, drive it with a real client on the other.
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        let running = server.serve(server_io).await.unwrap();
        running.waiting().await.unwrap();
    });

    let peer = ().serve(client_io).await.unwrap();

    // tools/list exposes the four tools.
    let tools = peer.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert!(names.contains(&"search_memory".to_string()));
    assert!(names.contains(&"write_memory".to_string()));
    assert!(names.contains(&"upload_memory".to_string()));
    assert!(names.contains(&"read_memory".to_string()));

    // tools/call search_memory flows through to the backend and back.
    let mut params = CallToolRequestParams::default();
    params.name = "search_memory".into();
    params.arguments = serde_json::json!({ "query": "hello" }).as_object().cloned();
    let result = peer.call_tool(params).await.unwrap();
    let body = serde_json::to_string(&result).unwrap();
    assert!(body.contains("alice/abc/notes.md"), "got: {body}");
    assert!(body.contains("hello world"), "got: {body}");

    // tools/call write_memory flows model-authored content through to the backend.
    let mut params = CallToolRequestParams::default();
    params.name = "write_memory".into();
    params.arguments =
        serde_json::json!({ "filename": "decisions.md", "content": "we chose acktor" })
            .as_object()
            .cloned();
    let result = peer.call_tool(params).await.unwrap();
    let body = serde_json::to_string(&result).unwrap();
    assert!(body.contains("decisions.md"), "got: {body}");

    peer.cancel().await.ok();
    server_task.abort();
}

#[tokio::test]
async fn tool_call_surfaces_agent_id_resolution_failure() {
    // A backend that would succeed if reached. So if the tool proceeds with a bogus
    // (nil) agent_id instead of surfacing the resolution error, the call returns success
    // and the assertion below fails — pinning the no-silent-fallback behavior.
    let backend = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/memories/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "results": [] })),
        )
        .mount(&backend)
        .await;

    // No override forces on-disk resolution; pointing config_dir at a *file* makes
    // creating the `agents/` subdirectory fail.
    let tmp = tempfile::tempdir().unwrap();
    let config_dir_is_a_file = tmp.path().join("not_a_dir");
    std::fs::write(&config_dir_is_a_file, "x").unwrap();

    let config = Config {
        url: backend.uri(),
        token: "mh_tok".into(),
        agent_id: None,
    };
    let client = HttpClient::new(config.url.clone(), config.token.clone());
    let server = McpServer::new(client, config, config_dir_is_a_file);

    let (client_io, server_io) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        let running = server.serve(server_io).await.unwrap();
        running.waiting().await.unwrap();
    });
    let peer = ().serve(client_io).await.unwrap();

    let mut params = CallToolRequestParams::default();
    params.name = "search_memory".into();
    params.arguments = serde_json::json!({ "query": "hello" }).as_object().cloned();
    let result = peer.call_tool(params).await;

    // The resolution error must surface (as a protocol error or an error tool result),
    // not be swallowed by a nil-UUID fallback that quietly succeeds.
    let surfaced = match &result {
        Err(e) => format!("{e:?}"),
        Ok(r) => serde_json::to_string(r).unwrap(),
    };
    assert!(
        surfaced.contains("could not resolve agent id"),
        "expected the agent-id resolution error to surface, got: {surfaced}"
    );

    peer.cancel().await.ok();
    server_task.abort();
}
