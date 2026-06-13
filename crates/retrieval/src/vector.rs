//! `VectorStrategy`: brute-force cosine retrieval over cloud embeddings, with a built-in
//! `Bm25Strategy` it transparently falls back to when embeddings are unavailable (either the
//! index-time batch embed failed, or a per-query embed fails). The tool catalog is small, so
//! a linear scan over normalized vectors (cosine == dot product) is plenty.

use std::sync::Arc;

use async_trait::async_trait;
use catalog::Catalog;

use crate::embedder::Embedder;
use crate::{Bm25Strategy, RetrievalStrategy, ScoredTool};

/// L2-normalize in place; a zero vector is left as-is (its cosine with anything is 0).
fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The text embedded per tool: qualified name + description.
fn tool_text(t: &catalog::ToolDef) -> String {
    format!("{}\n{}", t.qualified_name(), t.description)
}

pub struct VectorStrategy {
    embedder: Arc<dyn Embedder>,
    bm25: Bm25Strategy,
    /// (qualified_name, description, normalized embedding) — empty when degraded.
    vectors: Vec<(String, String, Vec<f32>)>,
    degraded: bool,
}

impl VectorStrategy {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            embedder,
            bm25: Bm25Strategy::new(),
            vectors: Vec::new(),
            degraded: false,
        }
    }
}

#[async_trait]
impl RetrievalStrategy for VectorStrategy {
    async fn index(&mut self, catalog: &Catalog) {
        // Always (re)build the BM25 fallback first.
        self.bm25 = Bm25Strategy::new();
        self.bm25.index(catalog).await;

        let tools: Vec<&catalog::ToolDef> = catalog.iter().collect();
        let texts: Vec<String> = tools.iter().map(|t| tool_text(t)).collect();
        match self.embedder.embed(&texts).await {
            Ok(vecs) => {
                self.vectors = tools
                    .iter()
                    .zip(vecs)
                    .map(|(t, v)| (t.qualified_name(), t.description.clone(), normalize(v)))
                    .collect();
                self.degraded = false;
            }
            Err(e) => {
                tracing::warn!(error = %e, "vector index embedding failed; degrading to BM25");
                self.vectors.clear();
                self.degraded = true;
            }
        }
    }

    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        if self.degraded || self.vectors.is_empty() {
            return self.bm25.search(query, top_k).await;
        }
        let qv = match self.embedder.embed(&[query.to_string()]).await {
            Ok(mut v) => normalize(v.remove(0)),
            Err(e) => {
                tracing::warn!(error = %e, "vector query embedding failed; falling back to BM25");
                return self.bm25.search(query, top_k).await;
            }
        };

        let mut scored: Vec<ScoredTool> = self
            .vectors
            .iter()
            .map(|(qname, desc, v)| ScoredTool {
                qualified_name: qname.clone(),
                description: desc.clone(),
                score: dot(&qv, v),
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.qualified_name.cmp(&b.qualified_name))
        });
        scored.truncate(top_k);
        scored
    }
}
