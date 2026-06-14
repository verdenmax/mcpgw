//! `OpenAiChat`: a `ChatModel` backed by an OpenAI-compatible `/chat/completions` endpoint
//! (OpenAI, or local servers like Ollama/LM Studio/vLLM that speak the same shape). One of two
//! crates (with `embedder`) that depend on reqwest; everything else uses the `ChatModel` trait.

use std::time::Duration;

use async_trait::async_trait;
use retrieval::{ChatError, ChatModel};
use serde::Deserialize;

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

/// Calls `POST {base_url}/chat/completions` with a Bearer token and `temperature: 0` (stable
/// output). Returns `choices[0].message.content`.
pub struct OpenAiChat {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl OpenAiChat {
    pub fn new(
        base_url: String,
        model: String,
        api_key: String,
        timeout: Option<Duration>,
    ) -> Self {
        let mut builder = reqwest::Client::builder();
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        let client = builder.build().expect("reqwest client builds");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
        }
    }
}

#[async_trait]
impl ChatModel for OpenAiChat {
    async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError> {
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "temperature": 0,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            }))
            .send()
            .await
            .map_err(|e| ChatError::Provider(format!("request failed: {e}")))?;
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(500).collect();
            return Err(ChatError::Provider(format!(
                "HTTP {code} from chat endpoint: {snippet}"
            )));
        }
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| ChatError::Provider(format!("decode failed: {e}")))?;
        match parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
        {
            Some(s) if !s.trim().is_empty() => Ok(s),
            _ => Err(ChatError::Empty),
        }
    }
}
