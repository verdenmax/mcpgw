//! `CachingEmbedder`: an `Embedder` decorator that memoizes vectors by text content hash,
//! so repeated/unchanged tool texts are embedded only once across snapshot rebuilds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::embedder::{EmbedError, Embedder};

fn hash_text(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Memoizes embeddings by content hash. Only cache-miss texts are forwarded to `inner`.
pub struct CachingEmbedder {
    inner: Arc<dyn Embedder>,
    cache: Mutex<HashMap<u64, Arc<[f32]>>>,
}

impl CachingEmbedder {
    pub fn new(inner: Arc<dyn Embedder>) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Embedder for CachingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let hashes: Vec<u64> = texts.iter().map(|t| hash_text(t)).collect();

        // Collect unique cache-miss texts, preserving first-seen order.
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        {
            let cache = self.cache.lock().unwrap();
            for (h, t) in hashes.iter().zip(texts) {
                if !cache.contains_key(h) && miss_seen.insert(*h) {
                    miss_texts.push(t.clone());
                }
            }
        }

        // Embed only the misses (skip the call entirely if everything is cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            let mut cache = self.cache.lock().unwrap();
            for (t, v) in miss_texts.iter().zip(embedded) {
                cache.insert(hash_text(t), Arc::from(v.into_boxed_slice()));
            }
        }

        // Reassemble in original input order.
        let cache = self.cache.lock().unwrap();
        Ok(hashes
            .iter()
            .map(|h| cache.get(h).expect("just inserted/hit").to_vec())
            .collect())
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }
}
