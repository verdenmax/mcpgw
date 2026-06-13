use std::sync::Arc;

use catalog::Catalog;
use metatools::{call_tool, GatewaySnapshot, MetaError};
use retrieval::{Bm25Strategy, RetrievalStrategy};
use rmcp::ServiceExt;
use upstream::connection::UpstreamHandle;
use upstream::registry::UpstreamRegistry;
use upstream::testkit::MockUpstream;

/// Connect the in-memory mock under namespace `server`, ingest its tools into `catalog`,
/// and register the handle. Returns (snapshot, registry, server-join-handle).
async fn setup(
    server: &str,
) -> (
    GatewaySnapshot,
    UpstreamRegistry,
    tokio::task::JoinHandle<()>,
) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let join = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(server, client_io).await.unwrap();

    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();

    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(handle));

    let mut strat = Bm25Strategy::new();
    strat.index(&catalog).await;
    let snap = GatewaySnapshot::new(catalog, Box::new(strat));
    (snap, registry, join)
}

#[tokio::test]
async fn call_tool_routes_via_catalog_and_forwards() {
    let (snap, registry, join) = setup("mock").await;

    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("ping".into()));
    let result = call_tool(&snap, &registry, "mock__echo", Some(args))
        .await
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .expect("text content");
    assert_eq!(text, "ping");

    join.abort();
}

#[tokio::test]
async fn call_tool_unknown_tool_is_tool_not_found() {
    let (snap, registry, join) = setup("mock").await;
    let err = call_tool(&snap, &registry, "mock__nope", None)
        .await
        .unwrap_err();
    assert!(matches!(err, MetaError::ToolNotFound(_)), "got {err:?}");
    join.abort();
}

#[tokio::test]
async fn call_tool_unregistered_upstream_is_unavailable() {
    // The catalog references a server that has no live handle in the registry
    // (catalog/registry skew). Routing must report UpstreamUnavailable, not forward.
    let catalog = Catalog::from_tooldefs(vec![catalog::ToolDef {
        server: "ghost".into(),
        name: "do".into(),
        description: "nobody home".into(),
        input_schema: serde_json::Value::Null,
    }]);
    let mut strat = Bm25Strategy::new();
    strat.index(&catalog).await;
    let snap = GatewaySnapshot::new(catalog, Box::new(strat));
    let registry = UpstreamRegistry::new(); // empty — no "ghost" handle

    let err = call_tool(&snap, &registry, "ghost__do", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MetaError::UpstreamUnavailable(_)),
        "got {err:?}"
    );
}

#[tokio::test]
async fn call_tool_maps_upstream_timeout_to_metaerror_timeout() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let join = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect("mock", client_io)
        .await
        .unwrap()
        .with_call_timeout(std::time::Duration::from_millis(50));

    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();
    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(handle));
    let mut strat = Bm25Strategy::new();
    strat.index(&catalog).await;
    let snap = GatewaySnapshot::new(catalog, Box::new(strat));

    // mock__slow sleeps 10s; the 50ms handle timeout fires and maps to MetaError::Timeout.
    let err = call_tool(&snap, &registry, "mock__slow", None)
        .await
        .unwrap_err();
    assert!(matches!(err, MetaError::Timeout), "got {err:?}");

    join.abort();
}
