//! SubagentStrategy integration tests over the deterministic MockChatModel.
use std::sync::atomic::Ordering;
use std::sync::Arc;

use catalog::{Catalog, ToolDef};
use retrieval::{
    build_strategy, Backends, Bm25Strategy, MockChatModel, RetrievalStrategy, SubagentStrategy,
};
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
async fn rerank_follows_model_order() {
    // BM25 shortlist for "create github issue" = the two github tools; the model reorders them.
    let mock = Arc::new(MockChatModel::new(
        r#"["github__list_pull_requests", "github__create_issue"]"#,
    ));
    let mut s = SubagentStrategy::new(mock.clone(), 20);
    s.index(&sample()).await;
    let hits: Vec<String> = s
        .search("create github issue", 5)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(
        hits,
        vec![
            "github__list_pull_requests".to_string(),
            "github__create_issue".to_string()
        ]
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    assert!(mock.last_user.lock().unwrap().contains("Candidates:"));
}

#[tokio::test]
async fn hallucinated_names_are_dropped() {
    let mock = Arc::new(MockChatModel::new(
        r#"["nope__nonexistent", "github__create_issue"]"#,
    ));
    let mut s = SubagentStrategy::new(mock, 20);
    s.index(&sample()).await;
    let hits: Vec<String> = s
        .search("create github issue", 5)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(hits, vec!["github__create_issue".to_string()]);
}

#[tokio::test]
async fn degrades_to_bm25_when_chat_fails() {
    let mut s = SubagentStrategy::new(Arc::new(MockChatModel::failing()), 20);
    let cat = sample();
    s.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    let sq: Vec<String> = s
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    let bq: Vec<String> = b
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(sq, bq);
    assert!(!sq.is_empty());
}

#[tokio::test]
async fn garbage_reply_degrades_to_bm25() {
    let mut s = SubagentStrategy::new(Arc::new(MockChatModel::new("I think you want a tool")), 20);
    let cat = sample();
    s.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    let sq: Vec<String> = s
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    let bq: Vec<String> = b
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(sq, bq);
}

#[tokio::test]
async fn empty_shortlist_returns_empty_without_calling_chat() {
    let mock = Arc::new(MockChatModel::new(r#"["github__create_issue"]"#));
    let mut s = SubagentStrategy::new(mock.clone(), 20);
    s.index(&sample()).await;
    assert!(s.search("zzzznonexistent", 5).await.is_empty());
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn respects_top_k() {
    let mock = Arc::new(MockChatModel::new(
        r#"["github__create_issue", "github__list_pull_requests"]"#,
    ));
    let mut s = SubagentStrategy::new(mock, 20);
    s.index(&sample()).await;
    let hits = s.search("create github issue", 1).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].qualified_name, "github__create_issue");
}

#[tokio::test]
async fn build_strategy_subagent_with_chat_indexes_and_searches() {
    let backends = Backends {
        chat: Some(Arc::new(MockChatModel::new(r#"["weather__get_forecast"]"#))),
        ..Default::default()
    };
    let mut strat = build_strategy("subagent", &backends).expect("subagent ok with chat");
    strat.index(&sample()).await;
    let hits = strat.search("forecast", 5).await;
    assert_eq!(
        hits.first().map(|h| h.qualified_name.as_str()),
        Some("weather__get_forecast")
    );
}
