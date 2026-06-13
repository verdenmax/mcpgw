//! `HybridStrategy`: Reciprocal Rank Fusion (RRF) over a BM25 ranking and a vector ranking.
//!
//! Reuses `Bm25Strategy` (lexical) and `VectorStrategy` (semantic; itself self-degrades to
//! BM25 when the embedder is unavailable). RRF fuses by *rank*, so the two differently-scaled
//! score lists combine without normalization, and degradation self-heals: when the vector list
//! collapses to BM25 ranks, the fused order tracks BM25.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use catalog::Catalog;

use crate::embedder::Embedder;
use crate::{Bm25Strategy, RetrievalStrategy, ScoredTool, VectorStrategy};

/// Industry-standard RRF damping constant. Larger `k` flattens the rank weighting.
const RRF_K: f32 = 60.0;

/// Fuse ranked lists by Reciprocal Rank Fusion: each list contributes `1 / (RRF_K + rank)`
/// (rank from 1) to a document's score, summed by qualified name. Deterministic: ties break on
/// `qualified_name` ascending. Truncates to `top_k`.
fn rrf_fuse(lists: &[Vec<ScoredTool>], top_k: usize) -> Vec<ScoredTool> {
    // qualified_name -> (fused_score, description)
    let mut fused: HashMap<String, (f32, String)> = HashMap::new();
    for list in lists {
        for (i, hit) in list.iter().enumerate() {
            let rank = (i + 1) as f32;
            let contrib = 1.0 / (RRF_K + rank);
            let entry = fused
                .entry(hit.qualified_name.clone())
                .or_insert_with(|| (0.0, hit.description.clone()));
            entry.0 += contrib;
        }
    }
    let mut scored: Vec<ScoredTool> = fused
        .into_iter()
        .map(|(qualified_name, (score, description))| ScoredTool {
            qualified_name,
            description,
            score,
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

/// RRF hybrid of BM25 + vector retrieval. Requires an `Embedder` (the vector arm); construct via
/// `build_strategy("hybrid", Some(embedder))` or directly with `new`.
pub struct HybridStrategy {
    bm25: Bm25Strategy,
    vector: VectorStrategy,
    doc_count: usize,
}

impl HybridStrategy {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            bm25: Bm25Strategy::new(),
            vector: VectorStrategy::new(embedder),
            doc_count: 0,
        }
    }
}

#[async_trait]
impl RetrievalStrategy for HybridStrategy {
    async fn index(&mut self, catalog: &Catalog) {
        self.bm25.index(catalog).await;
        self.vector.index(catalog).await;
        self.doc_count = catalog.iter().count();
    }

    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        if self.doc_count == 0 {
            return Vec::new();
        }
        // Full-depth sub-rankings: RRF needs each doc's true rank in each list, so we must not
        // pre-truncate to top_k (that would drop one-sided matches before fusion).
        let lb = self.bm25.search(query, self.doc_count).await;
        let lv = self.vector.search(query, self.doc_count).await;
        rrf_fuse(&[lb, lv], top_k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, score: f32) -> ScoredTool {
        ScoredTool {
            qualified_name: name.into(),
            description: format!("desc of {name}"),
            score,
        }
    }

    #[test]
    fn rrf_ranks_doc_high_in_both_lists_first() {
        // a: rank1 in both -> 2/61 ; b: rank2 in both -> 2/62. a first.
        let l1 = vec![hit("a", 9.0), hit("b", 8.0)];
        let l2 = vec![hit("a", 0.9), hit("b", 0.8)];
        let out = rrf_fuse(&[l1, l2], 10);
        assert_eq!(
            out.iter().map(|h| h.qualified_name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!((out[0].score - 2.0 / 61.0).abs() < 1e-6);
        assert!((out[1].score - 2.0 / 62.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_breaks_ties_on_qualified_name() {
        // "b" rank1 in list1 only; "a" rank1 in list2 only -> equal 1/61 -> name asc.
        let out = rrf_fuse(&[vec![hit("b", 1.0)], vec![hit("a", 1.0)]], 10);
        assert_eq!(
            out.iter().map(|h| h.qualified_name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!((out[0].score - 1.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_includes_doc_present_in_only_one_list() {
        // a: 1/61 + 1/61 ; c: 1/62 -> a first, c present.
        let out = rrf_fuse(&[vec![hit("a", 1.0)], vec![hit("a", 1.0), hit("c", 0.5)]], 10);
        assert_eq!(
            out.iter().map(|h| h.qualified_name.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn rrf_respects_top_k() {
        let out = rrf_fuse(&[vec![hit("a", 3.0), hit("b", 2.0), hit("c", 1.0)]], 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].qualified_name, "a");
        assert_eq!(out[1].qualified_name, "b");
    }

    #[test]
    fn rrf_empty_lists_yield_empty() {
        assert!(rrf_fuse(&[], 5).is_empty());
        assert!(rrf_fuse(&[Vec::new(), Vec::new()], 5).is_empty());
    }
}
