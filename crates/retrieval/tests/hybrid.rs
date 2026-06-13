//! Hybrid (RRF) integration tests over the deterministic MockEmbedder.
use std::sync::Arc;

use catalog::{Catalog, ToolDef};
use retrieval::{build_strategy, Bm25Strategy, Embedder, HybridStrategy, MockEmbedder, RetrievalStrategy};
use serde_json::Value;

fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
    ToolDef {
        server: server.into(),
        name: name.into(),
        description: desc.into(),
        input_schema: Value::Null,
    }
}

fn sample() -> Catalog {
    Catalog::from_tooldefs(vec![
        tool("github", "create_issue", "Create a new issue in a GitHub repository"),
        tool("github", "list_pull_requests", "List pull requests for a repository"),
        tool("slack", "post_message", "Send a chat message to a Slack channel"),
        tool("weather", "get_forecast", "Get the weather forecast for a location"),
    ])
}

#[tokio::test]
async fn hybrid_ranks_relevant_tool_first() {
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::new(64)));
    h.index(&sample()).await;
    let hits = h.search("create github issue", 3).await;
    assert!(!hits.is_empty());
    assert_eq!(hits[0].qualified_name, "github__create_issue");
    for w in hits.windows(2) {
        assert!(w[0].score >= w[1].score, "scores must be descending");
    }
}

#[tokio::test]
async fn hybrid_degrades_to_bm25_order_when_embedder_fails() {
    // failing embedder -> VectorStrategy.search returns its internal BM25 list, so both fused
    // lists are identical BM25 rankings -> hybrid order matches standalone BM25.
    let cat = sample();
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::failing(64)));
    h.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    let hq: Vec<String> = h.search("repository", 10).await.into_iter().map(|x| x.qualified_name).collect();
    let bq: Vec<String> = b.search("repository", 10).await.into_iter().map(|x| x.qualified_name).collect();
    assert_eq!(hq, bq);
    assert!(!hq.is_empty(), "query 'repository' matches at least one tool");
}

#[tokio::test]
async fn hybrid_empty_catalog_returns_empty() {
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::new(64)));
    h.index(&Catalog::new()).await;
    assert!(h.search("anything", 5).await.is_empty());
}

#[tokio::test]
async fn hybrid_surfaces_vector_candidates_when_bm25_empty() {
    // No lexical overlap: BM25 alone returns nothing, but the vector list ranks all docs, so
    // hybrid still returns candidates (semantic recall).
    let cat = sample();
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::new(64)));
    h.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    assert!(b.search("zzzznonexistent", 5).await.is_empty());
    assert!(!h.search("zzzznonexistent", 5).await.is_empty());
}

#[tokio::test]
async fn build_strategy_hybrid_with_embedder_indexes_and_searches() {
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(64));
    let mut strat = build_strategy("hybrid", Some(&embedder)).expect("hybrid ok with embedder");
    strat.index(&sample()).await;
    let hits = strat.search("forecast", 8).await;
    assert_eq!(
        hits.first().map(|h| h.qualified_name.as_str()),
        Some("weather__get_forecast")
    );
}
