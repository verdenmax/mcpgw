use std::sync::Arc;

use gateway::GatewayState;
use metatools::search_tools;
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
async fn rebuild_snapshot_ingests_registered_upstreams() {
    let state = GatewayState::new("bm25").unwrap();

    // Empty before any upstream.
    assert!(search_tools(&state.snapshot(), "echo", 5).is_empty());

    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    // After rebuild, the mock's namespaced tools are searchable.
    let hits = search_tools(&state.snapshot(), "echo", 5);
    assert!(hits.iter().any(|s| s.name == "mock__echo"), "hits: {hits:?}");

    join.abort();
}

#[tokio::test]
async fn old_snapshot_reader_is_unaffected_by_rebuild() {
    let state = GatewayState::new("bm25").unwrap();
    let old = state.snapshot(); // Arc to the empty snapshot

    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    // The previously-loaded snapshot still works and still reflects the OLD (empty) state.
    assert!(search_tools(&old, "echo", 5).is_empty());
    // A freshly-loaded snapshot reflects the new state.
    assert!(!search_tools(&state.snapshot(), "echo", 5).is_empty());

    join.abort();
}
