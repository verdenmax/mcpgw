//! HTTP serving of the gateway's 3 meta-tools over Streamable HTTP (axum + rmcp
//! `StreamableHttpService`). Bearer auth (M1-C T4) is layered on when keys are configured.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
};
use gateway::GatewayState;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use subtle::ConstantTimeEq;

use crate::GatewayServer;

#[derive(Clone)]
struct ApiKeys(Arc<Vec<String>>);

/// True if `presented` equals any configured key. Per-key compare is constant-time for
/// equal-length inputs (length mismatch short-circuits, leaking only length). No early
/// return across the key set.
fn key_authorized(keys: &[String], presented: &[u8]) -> bool {
    let mut matched = 0u8;
    for k in keys {
        matched |= k.as_bytes().ct_eq(presented).unwrap_u8();
    }
    matched == 1
}

fn presented_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

async fn require_api_key(State(keys): State<ApiKeys>, req: Request, next: Next) -> Response {
    match presented_bearer(&req) {
        Some(k) if key_authorized(&keys.0, k.as_bytes()) => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Build the axum router that serves the 3 meta-tools at `path`. When `api_keys` is
/// non-empty, a Bearer API-key auth layer is mounted; when empty, requests pass through
/// (relying on localhost binding).
pub fn build_router(
    state: Arc<GatewayState>,
    default_top_k: usize,
    path: &str,
    api_keys: Vec<String>,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
) -> axum::Router {
    let service = StreamableHttpService::new(
        move || {
            Ok(GatewayServer::new(
                state.clone(),
                default_top_k,
                sinks.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service(path, service);
    if api_keys.is_empty() {
        router
    } else {
        router.layer(from_fn_with_state(
            ApiKeys(Arc::new(api_keys)),
            require_api_key,
        ))
    }
}
