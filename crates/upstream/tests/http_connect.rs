//! e2e: connect to a real HTTP MCP upstream (rmcp StreamableHttpService + MockUpstream),
//! verifying tool ingestion, call forwarding, and that auth headers reach the upstream.

use std::sync::{Arc, Mutex};

use axum::{
    extract::{Request, State},
    middleware::{from_fn_with_state, Next},
    response::Response,
};
use catalog::Catalog;
use config::{UpstreamConfig, UpstreamTransport};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use upstream::connect::connect_http_upstream;
use upstream::testkit::MockUpstream;

type Headers = Arc<Mutex<Vec<(String, String)>>>;

async fn record_headers(State(store): State<Headers>, req: Request, next: Next) -> Response {
    if let Some(v) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        store
            .lock()
            .unwrap()
            .push(("authorization".to_string(), v.to_string()));
    }
    next.run(req).await
}

/// Spawn a mock HTTP MCP upstream; return (url, recorded-headers store).
async fn spawn_mock_http_upstream() -> (String, Headers) {
    let store: Headers = Arc::new(Mutex::new(Vec::new()));
    let service = StreamableHttpService::new(
        || Ok(MockUpstream::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(store.clone(), record_headers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{addr}/mcp"), store)
}

fn http_cfg(name: &str, url: &str) -> UpstreamConfig {
    std::env::set_var("MCPGW_T6_BEARER", "topsecret");
    UpstreamConfig {
        name: name.to_string(),
        call_timeout_ms: 5_000,
        transport: UpstreamTransport::Http {
            url: url.to_string(),
            bearer_env: Some("MCPGW_T6_BEARER".to_string()),
            headers: std::collections::HashMap::new(),
        },
    }
}

#[tokio::test]
async fn connects_http_upstream_ingests_and_calls_with_auth_header() {
    let (url, headers) = spawn_mock_http_upstream().await;
    let cfg = http_cfg("remote", &url);

    let handle = connect_http_upstream(&cfg, None).await.expect("connect");

    // Tools are ingested (namespaced).
    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();
    assert!(
        catalog.get("remote__echo").is_some(),
        "echo should be ingested"
    );

    // call_tool forwards and returns the echoed text.
    let mut args = serde_json::Map::new();
    args.insert("text".to_string(), serde_json::json!("hi-http"));
    let result = handle.call_tool("echo", Some(args)).await.unwrap();
    assert!(result.content[0]
        .as_text()
        .unwrap()
        .text
        .contains("hi-http"));

    // The upstream saw our Authorization: Bearer header. Scope the lock so the guard is
    // released before `shutdown()` — shutdown sends a session-terminate request that passes
    // back through `record_headers`, which would deadlock on this same mutex if still held.
    {
        let recorded = headers.lock().unwrap();
        assert!(
            recorded
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer topsecret"),
            "upstream should have received the bearer header, got: {recorded:?}"
        );
    }

    handle.shutdown().await;
}
