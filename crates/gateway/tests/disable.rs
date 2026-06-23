use std::sync::Arc;

use gateway::{DisableSet, GatewayState};
use metatools::{get_tool_details, search_tools};
use rmcp::ServiceExt;
use upstream::connection::UpstreamHandle;
use upstream::testkit::MockUpstream;

async fn connect_mock(name: &str) -> (UpstreamHandle, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let join = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    (handle, join)
}

#[tokio::test]
async fn disabled_upstream_is_skipped_at_ingest_and_hidden_then_restored() {
    let disabled = Arc::new(DisableSet::default());
    let state = GatewayState::new("bm25")
        .unwrap()
        .with_disabled(disabled.clone());
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));

    // Disable BEFORE rebuild: the upstream is not even ingested (absent from summary.ingested)
    // and its tools are unsearchable.
    disabled.disable_upstream("mock");
    let summary = state.rebuild_snapshot().await.unwrap();
    assert!(
        !summary.ingested.contains(&"mock".to_string()),
        "disabled upstream must not be ingested: {summary:?}"
    );
    assert!(search_tools(&state.snapshot(), "echo", 5).await.is_empty());

    // Re-enable + rebuild: tools come back (connection was preserved).
    disabled.enable_upstream("mock");
    let summary = state.rebuild_snapshot().await.unwrap();
    assert!(summary.ingested.contains(&"mock".to_string()));
    let hits = search_tools(&state.snapshot(), "echo", 5).await;
    assert!(
        hits.iter().any(|s| s.name == "mock__echo"),
        "hits: {hits:?}"
    );

    join.abort();
}

#[tokio::test]
async fn disabled_single_tool_is_hidden_but_siblings_remain() {
    let disabled = Arc::new(DisableSet::default());
    let state = GatewayState::new("bm25")
        .unwrap()
        .with_disabled(disabled.clone());
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));

    disabled.disable_tool("mock__echo");
    state.rebuild_snapshot().await.unwrap();

    let snap = state.snapshot();
    assert!(
        get_tool_details(&snap, "mock__echo").is_none(),
        "echo must be hidden"
    );
    assert!(
        get_tool_details(&snap, "mock__greet").is_some(),
        "greet must remain"
    );
    assert!(
        !search_tools(&snap, "echo", 5)
            .await
            .iter()
            .any(|s| s.name == "mock__echo"),
        "disabled tool must not be searchable"
    );

    join.abort();
}
