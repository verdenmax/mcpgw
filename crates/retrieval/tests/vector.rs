use catalog::{Catalog, ToolDef};
use retrieval::{MockEmbedder, RetrievalStrategy, VectorStrategy};
use serde_json::Value;
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
    use retrieval::{build_strategy, Embedder};
    let e: std::sync::Arc<dyn Embedder> = std::sync::Arc::new(MockEmbedder::new(32));
    let mut strat = build_strategy("vector", Some(&e)).expect("vector with embedder");
    strat.index(&catalog()).await;
    assert!(!strat.search("forecast", 5).await.is_empty());
}
