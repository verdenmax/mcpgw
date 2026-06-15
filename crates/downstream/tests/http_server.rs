use std::sync::Arc;

use gateway::GatewayState;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::json;
use upstream::connection::UpstreamHandle;
use upstream::testkit::MockUpstream;

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

async fn attach_mock(state: &GatewayState, name: &str) {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = MockUpstream::new().serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}

/// Bind the gateway's HTTP router on an ephemeral port; return the bound addr.
async fn spawn_http_gateway(state: Arc<GatewayState>, api_keys: Vec<String>) -> String {
    let sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> = Vec::new().into();
    let router = downstream::http::build_router(state, 8, "/mcp", api_keys, sinks);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn http_gateway_serves_search_details_call() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec![]).await;

    let client = ().serve(StreamableHttpClientTransport::from_uri(url)).await.unwrap();

    // list_tools -> exactly 3 meta-tools.
    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["call_tool", "get_tool_details", "search_tools"]);

    // search -> details -> call.
    let r = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query":"echo"}))),
        )
        .await
        .unwrap();
    assert!(r.content[0].as_text().unwrap().text.contains("mock__echo"));

    let r = client
        .call_tool(
            CallToolRequestParams::new("get_tool_details")
                .with_arguments(args(json!({"name":"mock__echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("echo"));

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

const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

async fn post_init(url: &str, bearer: Option<&str>) -> reqwest::StatusCode {
    let mut req = reqwest::Client::new()
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json");
    if let Some(b) = bearer {
        req = req.header("Authorization", format!("Bearer {b}"));
    }
    req.body(INIT_BODY).send().await.unwrap().status()
}

#[tokio::test]
async fn http_auth_rejects_missing_and_wrong_key() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec!["good-key".to_string()]).await;

    assert_eq!(
        post_init(&url, None).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_init(&url, Some("bad")).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    // 正确 key 不应是 401（会进入 MCP 协议层，返回 2xx）。
    assert_ne!(
        post_init(&url, Some("good-key")).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn http_auth_allows_valid_key_full_flow() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec!["good-key".to_string()]).await;

    let cfg = StreamableHttpClientTransportConfig::with_uri(url).auth_header("good-key");
    let client = ().serve(StreamableHttpClientTransport::from_config(cfg)).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 3);
    client.cancel().await.unwrap();
}
