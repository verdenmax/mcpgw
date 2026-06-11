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

use rmcp::model::CallToolRequestParams;
use serde_json::json;

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

#[tokio::test]
async fn call_tool_dispatches_all_three_metatools() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;
    let client = common::connect_to_gateway(state, 8).await;

    // search_tools finds echo.
    let r = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query":"echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    let text = r.content[0].as_text().unwrap().text.clone();
    assert!(text.contains("mock__echo"), "search result: {text}");

    // get_tool_details returns echo's def.
    let r = client
        .call_tool(
            CallToolRequestParams::new("get_tool_details")
                .with_arguments(args(json!({"name":"mock__echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("echo"));

    // call_tool forwards to upstream echo.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "mock__echo", "arguments": {"text": "hi"}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("hi"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn call_tool_unknown_meta_name_is_protocol_error() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let client = common::connect_to_gateway(state, 8).await;
    let err = client
        .call_tool(CallToolRequestParams::new("does_not_exist"))
        .await;
    assert!(err.is_err(), "unknown meta-tool must be a protocol error");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn call_tool_routes_missing_upstream_tool_to_iserror() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;
    let client = common::connect_to_gateway(state, 8).await;
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name":"mock__nope"}))),
        )
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true)); // MetaError::ToolNotFound -> isError
    client.cancel().await.unwrap();
}
