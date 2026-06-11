//! Shared e2e harness: run a GatewayServer over an in-memory duplex and return a
//! connected rmcp test client, plus helpers to attach a mock upstream.

use std::sync::Arc;

use gateway::GatewayState;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;

use downstream::GatewayServer;

/// Spawn a GatewayServer (over duplex) with the given state; return a connected client.
/// The server task is detached; the client drives the test.
pub async fn connect_to_gateway(
    state: Arc<GatewayState>,
    default_top_k: usize,
) -> RunningService<RoleClient, ()> {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = GatewayServer::new(state, default_top_k);
    tokio::spawn(async move {
        if let Ok(svc) = server.serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    ().serve(client_io).await.expect("client connects")
}
