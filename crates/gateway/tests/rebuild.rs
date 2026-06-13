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
    assert!(search_tools(&state.snapshot(), "echo", 5).await.is_empty());

    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    // After rebuild, the mock's namespaced tools are searchable.
    let hits = search_tools(&state.snapshot(), "echo", 5).await;
    assert!(
        hits.iter().any(|s| s.name == "mock__echo"),
        "hits: {hits:?}"
    );

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
    assert!(search_tools(&old, "echo", 5).await.is_empty());
    // A freshly-loaded snapshot reflects the new state.
    assert!(!search_tools(&state.snapshot(), "echo", 5).await.is_empty());

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

    let hits = search_tools(&state.snapshot(), "echo", 10).await;
    assert!(
        hits.iter().any(|s| s.name == "good__echo"),
        "hits: {hits:?}"
    );
    assert!(
        !hits.iter().any(|s| s.name.starts_with("broken__")),
        "broken upstream should have been skipped: {hits:?}"
    );

    good_join.abort();
}

// A server that initializes fine but whose list_tools never returns: used to verify
// per-ingest timeout isolates a "connected but silent" upstream during rebuild.
#[derive(Clone)]
struct StalledListServer;

impl rmcp::ServerHandler for StalledListServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::from_build_env())
    }

    async fn list_tools(
        &self,
        _r: Option<rmcp::model::PaginatedRequestParams>,
        _c: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        Ok(rmcp::model::ListToolsResult::with_all_items(vec![]))
    }
}

#[tokio::test]
async fn rebuild_isolates_an_upstream_that_hangs_during_ingest() {
    use std::time::Duration;
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = StalledListServer.serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = upstream::connection::UpstreamHandle::connect("hung", client_io)
        .await
        .unwrap()
        .with_call_timeout(Duration::from_millis(80));

    let state = gateway::GatewayState::new("bm25").unwrap();
    state.registry().insert(std::sync::Arc::new(handle));

    let summary = tokio::time::timeout(Duration::from_secs(5), state.rebuild_snapshot())
        .await
        .expect("rebuild must not hang on a stalled upstream")
        .unwrap();

    assert!(summary.ingested.is_empty());
    assert_eq!(summary.skipped.len(), 1);
    assert_eq!(summary.skipped[0].0, "hung");
}

#[tokio::test]
async fn rebuild_worker_rebuilds_when_triggered() {
    use std::time::Duration;
    let state = gateway::GatewayState::new("bm25").unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(8);
    let worker = tokio::spawn(gateway::run_rebuild_worker(state.clone(), rx));

    // Attach a mock upstream (but don't trigger a rebuild yet).
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        MockUpstream::new()
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let handle = UpstreamHandle::connect("mock", client_io).await.unwrap();
    state.registry().insert(std::sync::Arc::new(handle));

    // Before triggering: snapshot is empty (search finds nothing).
    assert!(metatools::search_tools(&state.snapshot(), "echo", 5)
        .await
        .is_empty());

    // Trigger one rebuild.
    tx.send("mock".to_string()).await.unwrap();

    // Poll until the snapshot reflects the mock's tools.
    let mut found = false;
    for _ in 0..100 {
        if metatools::search_tools(&state.snapshot(), "echo", 5)
            .await
            .iter()
            .any(|s| s.name == "mock__echo")
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        found,
        "worker should rebuild snapshot to include mock__echo"
    );

    drop(tx); // close channel -> worker exits
    let _ = worker.await;
}

#[tokio::test]
async fn with_embedder_drives_vector_ranking_through_gateway_state() {
    use retrieval::MockEmbedder;

    // Vector strategy backed by a deterministic embedder, driven end-to-end through the
    // gateway boundary: connect a non-empty mock upstream, rebuild, then search.
    let state =
        GatewayState::with_embedder("vector", Arc::new(MockEmbedder::new(64))).expect("vector");
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    // The query shares tokens with `mock__echo`'s text ("Echo the provided text back"), so
    // cosine ranking must surface it first ahead of greet/slow (zero token overlap).
    // `search_tools` returns hits already ordered best-first, so position 0 is the top rank.
    let hits = search_tools(&state.snapshot(), "echo provided text back", 5).await;
    assert!(!hits.is_empty(), "vector search returned no hits");
    assert_eq!(hits[0].name, "mock__echo", "hits: {hits:?}");

    join.abort();
}

#[tokio::test]
async fn caching_embedder_persists_across_rebuilds_via_gateway_state() {
    use retrieval::{CachingEmbedder, MockEmbedder};

    // Wrap the inner MockEmbedder in a CachingEmbedder and keep a handle on its call counter.
    let mock = MockEmbedder::new(64);
    let calls = mock.calls.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));
    let state =
        GatewayState::with_embedder("vector", Arc::new(caching)).expect("vector with caching");

    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));

    // First rebuild over the (cache-cold) catalog: the inner embedder is invoked.
    state.rebuild_snapshot().await.unwrap();
    let after_first = calls.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        after_first > 0,
        "inner embedder should run on the 1st rebuild"
    );

    // Second rebuild over the SAME unchanged catalog: every tool text is a cache hit, so the
    // inner MockEmbedder must NOT be re-invoked (cross-rebuild cache persistence).
    state.rebuild_snapshot().await.unwrap();
    let after_second = calls.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        after_second, after_first,
        "inner embedder re-invoked for unchanged tools on the 2nd rebuild"
    );

    join.abort();
}
