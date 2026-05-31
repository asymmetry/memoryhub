//! Thin HTTP client over the MemoryHub `/v1` API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Errors from a MemoryHub API call.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("authentication failed — check MEMORYHUB_TOKEN")]
    Unauthorized,

    #[error("cannot reach MemoryHub at {0}")]
    Unreachable(String),

    #[error("MemoryHub returned {status}: {body}")]
    Http { status: u16, body: String },

    #[error("unexpected response from MemoryHub: {0}")]
    Decode(String),
}

#[derive(Debug, Serialize)]
struct WriteRequest<'a> {
    agent_id: Uuid,
    filename: &'a str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct ReadRequest<'a> {
    agent_id: Uuid,
    filename: &'a str,
}

#[derive(Debug, Deserialize)]
struct ReadResponse {
    content: String,
}

#[derive(Debug, Serialize)]
struct SearchRequest<'a> {
    agent_id: Uuid,
    query: &'a str,
}

/// One search hit (mirrors the server's `SearchResult`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SearchResult {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub snippet: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

/// Client holding the base URL, bearer token, and a shared `reqwest::Client`.
#[derive(Clone)]
pub struct MemoryClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl MemoryClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
            token,
        }
    }

    async fn post<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<reqwest::Response, ClientError> {
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|_| ClientError::Unreachable(self.base_url.clone()))?;
        if resp.status().as_u16() == 401 {
            return Err(ClientError::Unauthorized);
        }
        Ok(resp)
    }

    async fn ensure_ok(resp: reqwest::Response) -> Result<reqwest::Response, ClientError> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Http { status, body })
        }
    }

    pub async fn search(
        &self,
        agent_id: Uuid,
        query: &str,
    ) -> Result<Vec<SearchResult>, ClientError> {
        let resp = self
            .post("/v1/memories/search", &SearchRequest { agent_id, query })
            .await?;
        let resp = Self::ensure_ok(resp).await?;
        let parsed: SearchResponse = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(parsed.results)
    }

    pub async fn write(
        &self,
        agent_id: Uuid,
        filename: &str,
        content: &str,
    ) -> Result<(), ClientError> {
        let resp = self
            .post(
                "/v1/memories/write",
                &WriteRequest {
                    agent_id,
                    filename,
                    content,
                },
            )
            .await?;
        Self::ensure_ok(resp).await?;
        Ok(())
    }

    /// Returns `None` when the file does not exist (404).
    pub async fn read(
        &self,
        agent_id: Uuid,
        filename: &str,
    ) -> Result<Option<String>, ClientError> {
        let resp = self
            .post("/v1/memories/read", &ReadRequest { agent_id, filename })
            .await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = Self::ensure_ok(resp).await?;
        let parsed: ReadResponse = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(Some(parsed.content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn agent() -> Uuid {
        Uuid::new_v4()
    }

    #[tokio::test]
    async fn search_parses_results_and_sends_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/search"))
            .and(header("authorization", "Bearer mh_tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [{
                    "path": "alice/abc/notes.md",
                    "start_line": 1, "end_line": 3, "score": 0.9, "snippet": "hello"
                }]
            })))
            .mount(&server)
            .await;

        let client = MemoryClient::new(server.uri(), "mh_tok".into());
        let results = client.search(agent(), "hello").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "alice/abc/notes.md");
    }

    #[tokio::test]
    async fn write_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/write"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let client = MemoryClient::new(server.uri(), "mh_tok".into());
        client.write(agent(), "notes.md", "hi").await.unwrap();
    }

    #[tokio::test]
    async fn read_some_and_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/read"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"content": "body"})),
            )
            .mount(&server)
            .await;
        let client = MemoryClient::new(server.uri(), "mh_tok".into());
        assert_eq!(
            client.read(agent(), "x.md").await.unwrap(),
            Some("body".to_string())
        );

        let server404 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/read"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"not_found"}"#))
            .mount(&server404)
            .await;
        let client404 = MemoryClient::new(server404.uri(), "mh_tok".into());
        assert_eq!(client404.read(agent(), "missing.md").await.unwrap(), None);
    }

    #[tokio::test]
    async fn unauthorized_maps_to_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/search"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"unauthorized"}"#))
            .mount(&server)
            .await;
        let client = MemoryClient::new(server.uri(), "bad".into());
        let err = client.search(agent(), "q").await.unwrap_err();
        assert!(matches!(err, ClientError::Unauthorized));
    }

    #[tokio::test]
    async fn unreachable_maps_to_error() {
        // Nothing listening on this port.
        let client = MemoryClient::new("http://127.0.0.1:1".into(), "t".into());
        let err = client.search(agent(), "q").await.unwrap_err();
        assert!(matches!(err, ClientError::Unreachable(_)));
    }
}
