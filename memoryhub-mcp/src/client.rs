//! Thin HTTP client over the MemoryHub `/v1` API.

use reqwest::Response;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ClientError;

#[derive(Debug, Serialize)]
struct WriteRequest<'a> {
    agent_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    filename: &'a str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct ReadRequest<'a> {
    agent_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    filename: &'a str,
}

#[derive(Debug, Deserialize)]
struct ReadResponse {
    content: String,
}

#[derive(Debug, Serialize)]
struct SearchRequest<'a> {
    agent_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    raw_only: bool,
    query: &'a str,
}

/// One search hit (mirrors `SearchResult` in `memoryhub`).
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

#[derive(Debug, Serialize)]
struct SummaryRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<Uuid>,
    scope: &'a str,
}

#[derive(Debug, Deserialize)]
struct SummaryResponse {
    content: String,
}

/// Client holding the base URL, bearer token, and a shared `reqwest::Client`.
#[derive(Clone)]
pub struct MemoryHubClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl MemoryHubClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
            token,
        }
    }

    async fn post<B>(&self, path: &str, body: &B) -> Result<Response, ClientError>
    where
        B: Serialize,
    {
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

    async fn ensure_ok(resp: Response) -> Result<Response, ClientError> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Http { status, body })
        }
    }

    /// Proxies a `search` to the MemoryHub API.
    pub async fn search(
        &self,
        agent_id: Uuid,
        scope: Option<&str>,
        raw_only: bool,
        query: &str,
    ) -> Result<Vec<SearchResult>, ClientError> {
        let resp = self
            .post(
                "/v1/memories/search",
                &SearchRequest {
                    agent_id,
                    scope,
                    raw_only,
                    query,
                },
            )
            .await?;
        let resp = Self::ensure_ok(resp).await?;
        let parsed: SearchResponse = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(parsed.results)
    }

    /// Proxies a `write` to the MemoryHub API.
    pub async fn write(
        &self,
        agent_id: Uuid,
        project: Option<&str>,
        filename: &str,
        content: &str,
    ) -> Result<(), ClientError> {
        let resp = self
            .post(
                "/v1/memories/write",
                &WriteRequest {
                    agent_id,
                    project,
                    filename,
                    content,
                },
            )
            .await?;
        Self::ensure_ok(resp).await?;
        Ok(())
    }

    /// Proxies a `read` to the MemoryHub API.
    pub async fn read(
        &self,
        agent_id: Uuid,
        project: Option<&str>,
        filename: &str,
    ) -> Result<Option<String>, ClientError> {
        let resp = self
            .post(
                "/v1/memories/read",
                &ReadRequest {
                    agent_id,
                    project,
                    filename,
                },
            )
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

    /// Proxies a `summary` to the MemoryHub API; 404 → `None`.
    pub async fn summary(
        &self,
        agent_id: Option<Uuid>,
        scope: &str,
    ) -> Result<Option<String>, ClientError> {
        let resp = self
            .post("/v1/memories/summary", &SummaryRequest { agent_id, scope })
            .await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = Self::ensure_ok(resp).await?;
        let parsed: SummaryResponse = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(Some(parsed.content))
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    use super::*;

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

        let client = MemoryHubClient::new(server.uri(), "mh_tok".into());
        let results = client.search(agent(), None, false, "hello").await.unwrap();
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
        let client = MemoryHubClient::new(server.uri(), "mh_tok".into());
        client.write(agent(), None, "notes.md", "hi").await.unwrap();
    }

    #[tokio::test]
    async fn read_some_and_none() {
        use wiremock::matchers::body_partial_json;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/read"))
            .and(body_partial_json(serde_json::json!({ "project": "notes" })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"content": "body"})),
            )
            .mount(&server)
            .await;
        let client = MemoryHubClient::new(server.uri(), "mh_tok".into());
        assert_eq!(
            client.read(agent(), Some("notes"), "x.md").await.unwrap(),
            Some("body".to_string())
        );

        let server404 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/read"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"not_found"}"#))
            .mount(&server404)
            .await;
        let client404 = MemoryHubClient::new(server404.uri(), "mh_tok".into());
        assert_eq!(
            client404.read(agent(), None, "missing.md").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn unauthorized_maps_to_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/search"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"unauthorized"}"#))
            .mount(&server)
            .await;
        let client = MemoryHubClient::new(server.uri(), "bad".into());
        let err = client.search(agent(), None, false, "q").await.unwrap_err();
        assert!(matches!(err, ClientError::Unauthorized));
    }

    #[tokio::test]
    async fn unreachable_maps_to_error() {
        // Nothing listening on this port.
        let client = MemoryHubClient::new("http://127.0.0.1:1".into(), "t".into());
        let err = client.search(agent(), None, false, "q").await.unwrap_err();
        assert!(matches!(err, ClientError::Unreachable(_)));
    }

    #[test]
    fn summary_request_omits_absent_agent_id() {
        let body = serde_json::to_string(&SummaryRequest {
            agent_id: None,
            scope: "global",
        })
        .unwrap();
        assert!(
            !body.contains("agent_id"),
            "agent_id should be omitted, got: {body}"
        );
        assert!(body.contains("\"scope\":\"global\""), "got: {body}");
    }

    #[tokio::test]
    async fn summary_some_and_none() {
        use wiremock::matchers::body_partial_json;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/summary"))
            .and(body_partial_json(serde_json::json!({ "scope": "user" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"content": "digest", "path": "alice/_synthesized/2026-06-03-01.md"}),
            ))
            .mount(&server)
            .await;
        let client = MemoryHubClient::new(server.uri(), "mh_tok".into());
        assert_eq!(
            client.summary(Some(agent()), "user").await.unwrap(),
            Some("digest".to_string())
        );

        let server404 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/memories/summary"))
            .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"not_found"}"#))
            .mount(&server404)
            .await;
        let client404 = MemoryHubClient::new(server404.uri(), "mh_tok".into());
        assert_eq!(
            client404.summary(Some(agent()), "user").await.unwrap(),
            None
        );
    }
}
