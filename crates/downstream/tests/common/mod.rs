//! Shared e2e harness: run a GatewayServer over an in-memory duplex and return a
//! connected rmcp test client, plus helpers to attach a mock upstream.

use std::sync::Arc;

use gateway::GatewayState;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;

use downstream::GatewayServer;
use upstream::connection::UpstreamHandle;
use upstream::testkit::MockUpstream;
use upstream::testkit::RevealingMockUpstream;

/// Spawn a GatewayServer (over duplex) with the given state; return a connected client.
/// The server task is detached; the client drives the test.
pub async fn connect_to_gateway(
    state: Arc<GatewayState>,
    default_top_k: usize,
) -> RunningService<RoleClient, ()> {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = GatewayServer::new(state, default_top_k);
    tokio::spawn(async move {
        let svc = server
            .serve(server_io)
            .await
            .expect("gateway server serves");
        let _ = svc.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

/// Attach a MockUpstream (echo/greet/slow) into the state's registry under `name`,
/// then rebuild the snapshot so its tools are searchable/callable.
pub async fn attach_mock(state: &GatewayState, name: &str) {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        let svc = MockUpstream::new()
            .serve(server_io)
            .await
            .expect("mock upstream serves");
        let _ = svc.waiting().await;
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    state.registry().insert(std::sync::Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}

/// Attach a RevealingMockUpstream WITH a list_changed trigger, spawn the gateway's rebuild
/// worker, and build the initial snapshot. The worker lives for the duration of the test.
pub async fn attach_revealing_mock_with_worker(state: &Arc<GatewayState>, name: &str) {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(8);
    tokio::spawn(gateway::run_rebuild_worker((**state).clone(), rx));

    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = RevealingMockUpstream::new().serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = UpstreamHandle::connect_with_trigger(name, client_io, Some(tx))
        .await
        .unwrap();
    state.registry().insert(std::sync::Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}
