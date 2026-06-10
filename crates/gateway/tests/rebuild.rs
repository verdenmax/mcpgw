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

#[tokio::test]
async fn new_rejects_unimplemented_strategy() {
    assert!(GatewayState::new("vector").is_err());
    assert!(GatewayState::new("bm25").is_ok());
}

#[tokio::test]
async fn rebuild_isolates_a_failed_upstream() {
    let state = GatewayState::new("bm25").unwrap();

    // A healthy upstream and a "broken" one (its server is aborted before the rebuild, so
    // ingest_into hits EOF and fails). The healthy upstream must still be ingested.
    let (good, good_join) = connect_mock("good").await;
    let (broken, broken_join) = connect_mock("broken").await;
    // Kill the broken upstream's server and WAIT for it to finish, so its duplex end is
    // closed before the rebuild — otherwise abort() is cooperative and the server could
    // still answer list_all_tools.
    broken_join.abort();
    let _ = broken_join.await;

    state.registry().insert(Arc::new(good));
    state.registry().insert(Arc::new(broken));
    state.rebuild_snapshot().await.unwrap(); // must not error despite the broken upstream

    let hits = search_tools(&state.snapshot(), "echo", 10);
    assert!(hits.iter().any(|s| s.name == "good__echo"), "hits: {hits:?}");
    assert!(
        !hits.iter().any(|s| s.name.starts_with("broken__")),
        "broken upstream should have been skipped: {hits:?}"
    );

    good_join.abort();
}
