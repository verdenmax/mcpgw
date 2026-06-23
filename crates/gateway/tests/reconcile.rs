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

fn ups(toml: &str) -> Vec<config::UpstreamConfig> {
    config::Config::from_toml_str(toml).unwrap().upstreams
}

fn trig() -> tokio::sync::mpsc::Sender<String> {
    tokio::sync::mpsc::channel::<String>(8).0
}

// Note: the successful add/(re)connect path can't be driven from this harness — `connect_all`
// establishes real stdio-child / HTTP connections, which the in-memory duplex MockUpstream can't
// satisfy. Connect success is covered by connect_all's own tests (upstream crate) + the live demo;
// here we cover remove, unchanged-no-op, and best-effort add-failure.

#[tokio::test]
async fn reconcile_removes_deleted_upstream_and_rebuilds() {
    let state = GatewayState::new("bm25").unwrap();
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
    assert!(search_tools(&state.snapshot(), "echo", 5)
        .await
        .iter()
        .any(|s| s.name == "mock__echo"));

    // old has "mock" (command irrelevant — removal is by name), new is empty.
    let old = ups("[[upstream]]\nname=\"mock\"\ntransport=\"stdio\"\ncommand=\"x\"\n");
    let summary = state.reconcile_upstreams(&old, &[], trig()).await;
    assert_eq!(summary.removed, vec!["mock"]);
    assert!(search_tools(&state.snapshot(), "echo", 5).await.is_empty());
    assert!(state.registry().get("mock").is_none());

    join.abort();
}

#[tokio::test]
async fn reconcile_noop_when_unchanged_keeps_connection() {
    let state = GatewayState::new("bm25").unwrap();
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    let cfg = ups("[[upstream]]\nname=\"mock\"\ntransport=\"stdio\"\ncommand=\"x\"\n");
    let summary = state.reconcile_upstreams(&cfg, &cfg, trig()).await; // identical -> no-op
    assert!(
        summary.removed.is_empty() && summary.added.is_empty() && summary.reconnected.is_empty()
    );
    // connection preserved -> tools still searchable after the rebuild
    assert!(search_tools(&state.snapshot(), "echo", 5)
        .await
        .iter()
        .any(|s| s.name == "mock__echo"));

    join.abort();
}

#[tokio::test]
async fn reconcile_add_failure_is_best_effort() {
    // A brand-new upstream whose stdio command can't spawn: connect fails, recorded in
    // connect_failures, no panic, rebuild still runs.
    let state = GatewayState::new("bm25").unwrap();
    let new = ups(
        "[[upstream]]\nname=\"bad\"\ntransport=\"stdio\"\ncommand=\"/nonexistent-mcpgw-bin\"\n",
    );
    let summary = state.reconcile_upstreams(&[], &new, trig()).await;
    assert_eq!(summary.added, vec!["bad"]);
    assert_eq!(summary.connect_failures.len(), 1);
    assert_eq!(summary.connect_failures[0].0, "bad");
    assert!(state.registry().get("bad").is_none()); // failed connect not inserted
}
