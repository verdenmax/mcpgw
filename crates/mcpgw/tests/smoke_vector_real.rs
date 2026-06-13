//! Gated real vector smoke: embed the tool catalog with a real OpenAI-compatible endpoint and
//! assert a *semantic* query (no shared literal tokens) ranks the right tool first — something
//! BM25 cannot do. #[ignore]d; needs OPENAI_API_KEY (+ optional MCPGW_EMBED_BASE_URL / _MODEL).
//!
//! Run: cargo test -p mcpgw --test smoke_vector_real -- --ignored --nocapture

use std::sync::Arc;

use catalog::{Catalog, ToolDef};
use embedder::OpenAiEmbedder;
use retrieval::{RetrievalStrategy, VectorStrategy};
use serde_json::Value;

fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
    ToolDef {
        server: server.into(),
        name: name.into(),
        description: desc.into(),
        input_schema: Value::Null,
    }
}

#[tokio::test]
#[ignore = "real embeddings: needs OPENAI_API_KEY (+ optional MCPGW_EMBED_BASE_URL/_MODEL)"]
async fn semantic_query_ranks_right_tool_first() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("skipping: OPENAI_API_KEY not set");
        return;
    };
    let base = std::env::var("MCPGW_EMBED_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".into());
    let model =
        std::env::var("MCPGW_EMBED_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    let catalog = Catalog::from_tooldefs(vec![
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
        tool(
            "filesystem",
            "write_file",
            "Write contents to a file on disk",
        ),
    ]);
    let embedder = Arc::new(OpenAiEmbedder::new(base, model, key, None, None));
    let mut strat = VectorStrategy::new(embedder);
    strat.index(&catalog).await;

    // "communicate with my team" shares no literal token with any tool description, so BM25
    // would return nothing; vector retrieval should still rank Slack first.
    let hits = strat.search("communicate with my team", 4).await;
    assert_eq!(
        hits.first().map(|h| h.qualified_name.as_str()),
        Some("slack__post_message"),
        "semantic top-1 should be slack__post_message, got: {hits:?}"
    );
}
