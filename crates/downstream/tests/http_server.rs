use std::sync::Arc;

use gateway::GatewayState;
use rmcp::model::CallToolRequestParams;
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
    let router = downstream::http::build_router(state, 8, "/mcp", api_keys);
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
