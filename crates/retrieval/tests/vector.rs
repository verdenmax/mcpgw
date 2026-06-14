use catalog::{Catalog, ToolDef};
use retrieval::{EmbedError, Embedder, MockEmbedder, RetrievalStrategy, VectorStrategy};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
    ToolDef {
        server: server.into(),
        name: name.into(),
        description: desc.into(),
        input_schema: Value::Null,
    }
}

fn catalog() -> Catalog {
    Catalog::from_tooldefs(vec![
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
        tool(
            "github",
            "create_issue",
            "Create a new issue in a GitHub repository",
        ),
    ])
}

#[tokio::test]
async fn ranks_by_cosine_similarity() {
    let mut s = VectorStrategy::new(Arc::new(MockEmbedder::new(128)));
    s.index(&catalog()).await;
    let hits = s.search("send chat message slack channel", 3).await;
    assert_eq!(hits[0].qualified_name, "slack__post_message");
    // sorted descending
    for w in hits.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
}

#[tokio::test]
async fn truncates_to_top_k() {
    let mut s = VectorStrategy::new(Arc::new(MockEmbedder::new(64)));
    s.index(&catalog()).await;
    assert_eq!(s.search("message", 1).await.len(), 1);
}

#[tokio::test]
async fn degrades_to_bm25_when_index_embedding_fails() {
    let mut s = VectorStrategy::new(Arc::new(MockEmbedder::failing(64)));
    s.index(&catalog()).await; // embed fails at index -> degraded, BM25 still built
    let hits = s.search("forecast", 5).await; // served by the built-in BM25
    assert_eq!(hits[0].qualified_name, "weather__get_forecast");
}

#[tokio::test]
async fn build_strategy_vector_with_embedder_works() {
    use retrieval::{build_strategy, Backends, Embedder};
    let e: std::sync::Arc<dyn Embedder> = std::sync::Arc::new(MockEmbedder::new(32));
    let mut strat = build_strategy(
        "vector",
        &Backends {
            embedder: Some(e.clone()),
            ..Default::default()
        },
    )
    .expect("vector with embedder");
    strat.index(&catalog()).await;
    assert!(!strat.search("forecast", 5).await.is_empty());
}

/// A `MockEmbedder`-backed embedder that injects failures on chosen (0-based) call indices,
/// so we can model "succeeds at index, fails on a later per-query embed" (and the reverse).
struct FlakyEmbedder {
    inner: MockEmbedder,
    calls: AtomicUsize,
    fail_on: Vec<usize>,
}

impl FlakyEmbedder {
    fn new(dim: usize, fail_on: Vec<usize>) -> Self {
        Self {
            inner: MockEmbedder::new(dim),
            calls: AtomicUsize::new(0),
            fail_on,
        }
    }
}

#[async_trait::async_trait]
impl Embedder for FlakyEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_on.contains(&n) {
            return Err(EmbedError::Provider("scripted failure".into()));
        }
        self.inner.embed(texts).await
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
}

#[tokio::test]
async fn per_query_embed_failure_falls_back_to_bm25() {
    // Call 0 = index (succeeds), call 1 = the per-query embed during search (fails).
    let mut s = VectorStrategy::new(Arc::new(FlakyEmbedder::new(64, vec![1])));
    s.index(&catalog()).await; // indexed with real vectors (not degraded)
                               // The per-query embed fails -> transparent fallback to the built-in BM25 (not empty/error).
    let hits = s.search("forecast", 5).await;
    assert!(!hits.is_empty());
    assert_eq!(hits[0].qualified_name, "weather__get_forecast");
}

#[tokio::test]
async fn degraded_then_reindex_recovers_vector_results() {
    // Call 0 = first index (fails -> degraded); subsequent calls succeed.
    let mut s = VectorStrategy::new(Arc::new(FlakyEmbedder::new(128, vec![0])));
    s.index(&catalog()).await; // embed fails -> degraded, BM25 fallback active
    let bm25_hits = s.search("forecast", 5).await;
    assert_eq!(bm25_hits[0].qualified_name, "weather__get_forecast");

    // Re-index with the now-working embedder: degraded flag cleared, vectors rebuilt.
    s.index(&catalog()).await;
    let hits = s.search("send chat message slack channel", 3).await;
    assert_eq!(hits[0].qualified_name, "slack__post_message");
    for w in hits.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
}
