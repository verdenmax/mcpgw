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
    let value = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token.to_string())
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
    discovery: Arc<[Arc<dyn observe::DiscoverySink>]>,
) -> axum::Router {
    let service = StreamableHttpService::new(
        move || {
            Ok(GatewayServer::new(
                state.clone(),
                default_top_k,
                sinks.clone(),
                discovery.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn req_with_auth(value: &str) -> Request {
        axum::http::Request::builder()
            .header(AUTHORIZATION, value)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn presented_bearer_extracts_token() {
        assert_eq!(
            presented_bearer(&req_with_auth("Bearer sk-123")),
            Some("sk-123".to_string())
        );
    }

    #[test]
    fn presented_bearer_treats_empty_token_as_absent() {
        // "Bearer " splits into scheme="Bearer", token="" -> empty token is treated as not presented.
        assert_eq!(presented_bearer(&req_with_auth("Bearer ")), None);
    }

    #[test]
    fn presented_bearer_scheme_is_case_insensitive() {
        for header in [
            "Bearer sk-123",
            "bearer sk-123",
            "BEARER sk-123",
            "BeArEr sk-123",
        ] {
            assert_eq!(
                presented_bearer(&req_with_auth(header)),
                Some("sk-123".to_string()),
                "scheme name must be case-insensitive: {header:?}"
            );
        }
    }

    #[test]
    fn presented_bearer_rejects_other_schemes() {
        for header in ["Basic sk-123", "Token sk-123", "sk-123", "Bearersk-123"] {
            assert_eq!(
                presented_bearer(&req_with_auth(header)),
                None,
                "non-bearer scheme must be rejected: {header:?}"
            );
        }
    }

    #[test]
    fn presented_bearer_token_value_stays_case_sensitive() {
        // Only the scheme is case-insensitive; the token itself is returned verbatim.
        assert_eq!(
            presented_bearer(&req_with_auth("bearer SK-Abc")),
            Some("SK-Abc".to_string())
        );
    }
}
