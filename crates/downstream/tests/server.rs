mod common;

use std::sync::Arc;

use gateway::GatewayState;

#[tokio::test]
async fn list_tools_returns_exactly_the_three_metatools() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let client = common::connect_to_gateway(state, 8).await;

    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["call_tool", "get_tool_details", "search_tools"]);

    client.cancel().await.unwrap();
}
