use async_trait::async_trait;
use catalog::Catalog;

mod caching;
mod chat;
mod embedder;
mod hybrid;
mod subagent;
mod vector;
pub use caching::CachingEmbedder;
#[cfg(feature = "testkit")]
pub use chat::MockChatModel;
pub use chat::{ChatError, ChatModel};
#[cfg(feature = "testkit")]
pub use embedder::MockEmbedder;
pub use embedder::{EmbedError, Embedder};
pub use hybrid::HybridStrategy;
pub use subagent::SubagentStrategy;
pub use vector::VectorStrategy;

/// A retrieval hit: a tool's qualified name, its description, and a relevance score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredTool {
    pub qualified_name: String,
    pub description: String,
    pub score: f32,
}

/// A pluggable tool-retrieval strategy (BM25, vector, hybrid, ...).
#[async_trait]
pub trait RetrievalStrategy: Send + Sync {
    /// (Re)build internal indices from the current catalog.
    async fn index(&mut self, catalog: &Catalog);
    /// Return up to `top_k` tools relevant to `query`, best first.
    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}

/// Lowercase, split on any non-alphanumeric boundary (this also splits `_`), drop empties.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

use std::collections::HashMap;

/// A single indexed document (one per tool). The searchable text is
/// the tool's qualified name plus its description.
#[derive(Debug, Clone)]
struct IndexedDoc {
    qualified_name: String,
    description: String,
    len: usize,
    term_freq: HashMap<String, u32>,
}

/// In-house BM25 ranking over the (small) tool catalog.
///
/// Deterministic and dependency-free, ideal for golden tests. For very large
/// catalogs, swap in a `tantivy`-backed strategy behind the same trait.
#[derive(Debug, Clone)]
pub struct Bm25Strategy {
    k1: f32,
    b: f32,
    docs: Vec<IndexedDoc>,
    doc_freq: HashMap<String, u32>,
    avgdl: f32,
    n: usize,
}

impl Bm25Strategy {
    pub fn new() -> Self {
        Self {
            k1: 1.2,
            b: 0.75,
            docs: Vec::new(),
            doc_freq: HashMap::new(),
            avgdl: 0.0,
            n: 0,
        }
    }

    fn idf(&self, term: &str) -> f32 {
        let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
        // BM25 idf with +1 to keep it non-negative.
        (((self.n as f32 - df + 0.5) / (df + 0.5)) + 1.0).ln()
    }
}

impl Default for Bm25Strategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RetrievalStrategy for Bm25Strategy {
    async fn index(&mut self, catalog: &Catalog) {
        let mut docs = Vec::new();
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        let mut total_len = 0usize;

        for tool in catalog.iter() {
            let mut text = tool.qualified_name();
            text.push(' ');
            text.push_str(&tool.description);
            let tokens = tokenize(&text);

            let mut term_freq: HashMap<String, u32> = HashMap::new();
            for tok in &tokens {
                *term_freq.entry(tok.clone()).or_insert(0) += 1;
            }
            for term in term_freq.keys() {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }

            total_len += tokens.len();
            docs.push(IndexedDoc {
                qualified_name: tool.qualified_name(),
                description: tool.description.clone(),
                len: tokens.len(),
                term_freq,
            });
        }

        self.n = docs.len();
        self.avgdl = if self.n == 0 {
            0.0
        } else {
            total_len as f32 / self.n as f32
        };
        self.doc_freq = doc_freq;
        self.docs = docs;
    }

    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        if self.n == 0 || self.avgdl == 0.0 {
            return Vec::new();
        }
        let q_terms = tokenize(query);

        let mut scored: Vec<ScoredTool> = self
            .docs
            .iter()
            .map(|doc| {
                let mut score = 0.0f32;
                for term in &q_terms {
                    if let Some(&f) = doc.term_freq.get(term) {
                        let f = f as f32;
                        let denom =
                            f + self.k1 * (1.0 - self.b + self.b * (doc.len as f32 / self.avgdl));
                        score += self.idf(term) * (f * (self.k1 + 1.0)) / denom;
                    }
                }
                ScoredTool {
                    qualified_name: doc.qualified_name.clone(),
                    description: doc.description.clone(),
                    score,
                }
            })
            // Keep only docs that share a query term. This `> 0.0` test is only a correct
            // "matched at all" check because `idf()` is always > 0 (the `+ 1.0` inside the ln
            // guarantees it): every contributing term adds a strictly positive amount, and
            // non-matching docs keep their initial 0.0. If `idf()` is ever changed to allow
            // negative values (e.g. the textbook BM25 idf), this filter would start dropping
            // legitimately-matched docs — keep that invariant in sync.
            .filter(|s| s.score > 0.0)
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

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("retrieval strategy {0:?} is not implemented in this version")]
    NotImplemented(String),
    #[error("retrieval strategy {0:?} requires an embedder but none was configured")]
    EmbedderRequired(String),
    #[error("retrieval strategy {0:?} requires a chat model but none was configured")]
    ChatModelRequired(String),
}

/// Optional retrieval backends injected into `build_strategy`. Bundling them keeps the factory
/// signature stable as new backends are added. `bm25` needs none; `vector`/`hybrid` need
/// `embedder`; `subagent` needs `chat`.
#[derive(Default, Clone)]
pub struct Backends {
    pub embedder: Option<std::sync::Arc<dyn Embedder>>,
    pub chat: Option<std::sync::Arc<dyn ChatModel>>,
    /// Shortlist size for "subagent"'s BM25 prefilter (None -> default). Consumed by the subagent
    /// arm added in T3; carried here so the factory signature stays stable.
    pub subagent_candidates: Option<usize>,
}

/// Construct a retrieval strategy by name, wired with the given optional `backends`.
///
/// Takes a plain `&str` (not a config type) so this crate stays free of any dependency on
/// `config` — callers pass `cfg.retrieval.strategy.as_str()`.
pub fn build_strategy(
    name: &str,
    backends: &Backends,
) -> Result<Box<dyn RetrievalStrategy>, StrategyError> {
    match name {
        "bm25" => Ok(Box::new(Bm25Strategy::new())),
        "vector" => match backends.embedder.as_ref() {
            Some(e) => Ok(Box::new(VectorStrategy::new(e.clone()))),
            None => Err(StrategyError::EmbedderRequired(name.to_string())),
        },
        "hybrid" => match backends.embedder.as_ref() {
            Some(e) => Ok(Box::new(HybridStrategy::new(e.clone()))),
            None => Err(StrategyError::EmbedderRequired(name.to_string())),
        },
        "subagent" => match backends.chat.as_ref() {
            Some(c) => Ok(Box::new(SubagentStrategy::new(
                c.clone(),
                backends
                    .subagent_candidates
                    .unwrap_or(subagent::DEFAULT_CANDIDATES),
            ))),
            None => Err(StrategyError::ChatModelRequired(name.to_string())),
        },
        other => Err(StrategyError::NotImplemented(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_on_non_alphanumeric_and_lowercases() {
        assert_eq!(
            tokenize("GitHub__create_issue"),
            vec!["github", "create", "issue"]
        );
        assert_eq!(tokenize("  multiple,, spaces "), vec!["multiple", "spaces"]);
        assert!(tokenize("").is_empty());
    }

    use catalog::ToolDef;
    use serde_json::Value;

    fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
        ToolDef {
            server: server.into(),
            name: name.into(),
            description: desc.into(),
            input_schema: Value::Null,
        }
    }

    fn sample_catalog() -> Catalog {
        Catalog::from_tooldefs(vec![
            tool(
                "github",
                "create_issue",
                "Create a new issue in a GitHub repository",
            ),
            tool(
                "github",
                "list_pull_requests",
                "List pull requests for a repository",
            ),
            tool(
                "slack",
                "post_message",
                "Send a chat message to a Slack channel",
            ),
            tool(
                "weather",
                "get_forecast",
                "Get the weather forecast for a location",
            ),
        ])
    }

    #[tokio::test]
    async fn bm25_ranks_relevant_tool_first() {
        let mut s = Bm25Strategy::new();
        s.index(&sample_catalog()).await;

        let hits = s.search("create github issue", 3).await;
        assert!(!hits.is_empty());
        assert_eq!(hits[0].qualified_name, "github__create_issue");
        // scores are sorted descending
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[tokio::test]
    async fn bm25_respects_top_k_and_filters_zero_score() {
        let mut s = Bm25Strategy::new();
        s.index(&sample_catalog()).await;

        // Only weather matches; top_k larger than match count returns just the match.
        let hits = s.search("forecast", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].qualified_name, "weather__get_forecast");

        // No term matches -> empty.
        assert!(s.search("zzzzz nonexistent", 10).await.is_empty());

        // top_k caps the result count.
        let capped = s.search("repository", 1).await;
        assert_eq!(capped.len(), 1);
    }

    #[tokio::test]
    async fn build_strategy_returns_bm25_and_indexes() {
        let mut strat = build_strategy("bm25", &Backends::default()).expect("bm25 is supported");
        strat.index(&sample_catalog()).await;
        let hits = strat.search("forecast", 8).await;
        assert_eq!(
            hits.first().map(|h| h.qualified_name.as_str()),
            Some("weather__get_forecast")
        );
    }

    #[test]
    fn build_strategy_errors_appropriately() {
        // hybrid without an embedder now errors as EmbedderRequired (was NotImplemented pre-M2-B).
        assert!(matches!(
            build_strategy("hybrid", &Backends::default()),
            Err(StrategyError::EmbedderRequired(_))
        ));
        assert!(matches!(
            build_strategy("vector", &Backends::default()),
            Err(StrategyError::EmbedderRequired(_))
        ));
        // A genuinely unknown name is still NotImplemented.
        assert!(matches!(
            build_strategy("nope", &Backends::default()),
            Err(StrategyError::NotImplemented(_))
        ));
        assert!(matches!(
            build_strategy("subagent", &Backends::default()),
            Err(StrategyError::ChatModelRequired(_))
        ));
    }
}
