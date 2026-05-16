//! `MockProvider` — a test `Provider` that records calls and replays scripted
//! responses. Used by tests in this crate and by integration tests.

use std::sync::Mutex;

use super::*;
use crate::llm::Embedding;

/// A `Provider` that records calls and replays scripted responses.
#[derive(Default)]
pub struct MockProvider {
    pub embed_replies: Mutex<Vec<Result<EmbedResult, LlmError>>>,
    pub chat_replies: Mutex<Vec<Result<ChatResponse, LlmError>>>,
    pub embed_calls: Mutex<Vec<Vec<String>>>,
    pub chat_calls: Mutex<Vec<Vec<ChatMessage>>>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            embed_replies: Mutex::new(Vec::new()),
            chat_replies: Mutex::new(Vec::new()),
            embed_calls: Mutex::new(Vec::new()),
            chat_calls: Mutex::new(Vec::new()),
        }
    }

    pub fn push_embed(&self, reply: Result<EmbedResult, LlmError>) {
        self.embed_replies.lock().unwrap().push(reply);
    }

    pub fn push_chat(&self, reply: Result<ChatResponse, LlmError>) {
        self.chat_replies.lock().unwrap().push(reply);
    }

    pub fn embed_call_count(&self) -> usize {
        self.embed_calls.lock().unwrap().len()
    }

    pub fn chat_call_count(&self) -> usize {
        self.chat_calls.lock().unwrap().len()
    }

    pub fn last_chat_call(&self) -> Option<Vec<ChatMessage>> {
        self.chat_calls.lock().unwrap().last().cloned()
    }

    pub fn canned_embed(dim: usize, count: usize, model: &str) -> EmbedResult {
        EmbedResult {
            model: model.to_string(),
            embeddings: (0..count).map(|_| Embedding(vec![0.1; dim])).collect(),
        }
    }
}

impl Provider for MockProvider {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<EmbedResult, LlmError>> + Send + 'a>> {
        self.embed_calls.lock().unwrap().push(texts.to_vec());
        let reply = self.embed_replies.lock().unwrap().pop().unwrap_or_else(|| {
            // Permissive default: zero vectors so unrelated tests that
            // don't care about embedding values still work.
            Ok(EmbedResult {
                model: "mock-default".into(),
                embeddings: texts.iter().map(|_| Embedding(vec![0.0; 4])).collect(),
            })
        });
        Box::pin(async move { reply })
    }

    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>> {
        self.chat_calls.lock().unwrap().push(messages.to_vec());
        let reply = self.chat_replies.lock().unwrap().pop().unwrap_or_else(|| {
            // Permissive default: a non-empty stub assistant reply.
            Ok(ChatResponse {
                model: "mock-default".into(),
                content: "[mock-default reply]".into(),
            })
        });
        Box::pin(async move { reply })
    }
}
