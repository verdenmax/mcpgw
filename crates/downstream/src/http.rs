//! HTTP serving of the gateway's 3 meta-tools over Streamable HTTP (axum + rmcp
//! `StreamableHttpService`). Bearer auth (M1-C T4) is layered on when keys are configured.

use std::sync::Arc;

use gateway::GatewayState;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use crate::GatewayServer;

/// Build the axum router that serves the 3 meta-tools at `path`. `api_keys` is accepted
/// now for a stable signature; Bearer enforcement is added in T4.
pub fn build_router(
    state: Arc<GatewayState>,
    default_top_k: usize,
    path: &str,
    _api_keys: Vec<String>,
) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(GatewayServer::new(state.clone(), default_top_k)),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    axum::Router::new().nest_service(path, service)
}
