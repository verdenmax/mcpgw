//! `OpenAiEmbedder`: an `Embedder` backed by an OpenAI-compatible `/embeddings` endpoint
//! (OpenAI, or local servers like Ollama/LM Studio/vLLM that speak the same shape). One of two
//! crates (with `chat`) that depend on reqwest; everything else uses the `Embedder` trait.

use std::time::Duration;

use async_trait::async_trait;
use retrieval::{EmbedError, Embedder};
use serde::Deserialize;

#[derive(Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

/// Calls `POST {base_url}/embeddings` with a Bearer token. `dim`, when set, is enforced.
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    dim: Option<usize>,
}

impl OpenAiEmbedder {
    pub fn new(
        base_url: String,
        model: String,
        api_key: String,
        dim: Option<usize>,
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
            dim,
        }
    }
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "model": self.model, "input": texts }))
            .send()
            .await
            .map_err(|e| EmbedError::Provider(format!("request failed: {e}")))?;
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(500).collect();
            return Err(EmbedError::Provider(format!(
                "HTTP {code} from embeddings endpoint: {snippet}"
            )));
        }
        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| EmbedError::Provider(format!("decode failed: {e}")))?;

        // Sort by `index` so output order matches input order regardless of server ordering.
        let mut data = parsed.data;
        data.sort_by_key(|d| d.index);
        if data.len() != texts.len() || data.iter().enumerate().any(|(i, d)| d.index != i) {
            return Err(EmbedError::Provider(format!(
                "embeddings response did not match {} inputs by index",
                texts.len()
            )));
        }
        if let Some(expected) = self.dim {
            for d in &data {
                if d.embedding.len() != expected {
                    return Err(EmbedError::Dimension {
                        expected,
                        got: d.embedding.len(),
                    });
                }
            }
        }
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }

    fn dim(&self) -> usize {
        self.dim.unwrap_or(0)
    }
}
