//! Admin write subsystem: Bearer-gated runtime disable/enable handlers + the auth middleware.
//! Gated-mount semantics: no admin token configured -> the middleware returns 404 (admin
//! effectively absent, existence not leaked); wrong/absent Bearer -> 401; match -> pass-through.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use gateway::DisableSet;
use subtle::ConstantTimeEq;

use crate::api::AppState;

/// Parse `Authorization: Bearer <token>` (scheme case-insensitive; empty token = absent).
fn presented_bearer(req: &Request) -> Option<String> {
    let raw = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[derive(Debug, PartialEq)]
enum AdminAuth {
    NotConfigured,
    Denied,
    Allowed,
}

/// Pure auth decision. No token configured -> NotConfigured (-> 404, no leak); configured +
/// matching Bearer -> Allowed; otherwise Denied (-> 401). Constant-time token compare (leaks only
/// length, mirroring the api-key path in downstream/src/http.rs).
fn authorize(configured: Option<&str>, presented: Option<&str>) -> AdminAuth {
    match configured {
        None => AdminAuth::NotConfigured,
        Some(expected) => match presented {
            Some(tok) if expected.as_bytes().ct_eq(tok.as_bytes()).into() => AdminAuth::Allowed,
            _ => AdminAuth::Denied,
        },
    }
}

/// Middleware on `/api/admin/*`: maps the auth decision to 404 / 401 / pass-through.
pub async fn require_admin_token(
    State(s): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let presented = presented_bearer(&req);
    match authorize(s.admin_token.as_deref(), presented.as_deref()) {
        AdminAuth::NotConfigured => StatusCode::NOT_FOUND.into_response(),
        AdminAuth::Denied => StatusCode::UNAUTHORIZED.into_response(),
        AdminAuth::Allowed => next.run(req).await,
    }
}

/// Run a disable-set mutation OFF the async worker — `DisableSet` persists synchronously (an
/// `fsync` under a lock), so doing it inline would block an axum executor thread. Then rebuild the
/// snapshot and return the updated set. (`DisableSet: Send + Sync` behind the `Arc`.) If the
/// rebuild fails the change is recorded but not yet reflected in the snapshot, so report 500 rather
/// than a misleading 200.
async fn mutate_and_rebuild(
    s: &Arc<AppState>,
    action: &'static str,
    name: String,
    mutate: impl FnOnce(&DisableSet) -> bool + Send + 'static,
) -> Response {
    let d = s.gateway.disabled_arc();
    let _ = tokio::task::spawn_blocking(move || mutate(&d)).await;
    if let Err(e) = s.gateway.rebuild_snapshot().await {
        tracing::warn!(action, name = %name, error = %e, "rebuild after admin disable-set change failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    tracing::info!(action, name = %name, "admin disable-set change applied");
    Json(s.gateway.disabled().snapshot()).into_response()
}

pub async fn disable_upstream(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    let dis = s.gateway.disabled();
    if dis.is_upstream_disabled(&name) {
        return Json(dis.snapshot()).into_response(); // idempotent no-op
    }
    if !s.upstreams.iter().any(|u| u.name == name) {
        return StatusCode::NOT_FOUND.into_response(); // unknown upstream
    }
    let nm = name.clone();
    mutate_and_rebuild(&s, "disable_upstream", name, move |ds| {
        ds.disable_upstream(&nm)
    })
    .await
}

pub async fn enable_upstream(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if !s.gateway.disabled().is_upstream_disabled(&name) {
        return Json(s.gateway.disabled().snapshot()).into_response();
    }
    let nm = name.clone();
    mutate_and_rebuild(&s, "enable_upstream", name, move |ds| {
        ds.enable_upstream(&nm)
    })
    .await
}

pub async fn disable_tool(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if s.gateway.disabled().is_tool_disabled(&name) {
        return Json(s.gateway.disabled().snapshot()).into_response();
    }
    if s.gateway.snapshot().catalog().get(&name).is_none() {
        return StatusCode::NOT_FOUND.into_response(); // tool not currently visible
    }
    let nm = name.clone();
    mutate_and_rebuild(&s, "disable_tool", name, move |ds| ds.disable_tool(&nm)).await
}

pub async fn enable_tool(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if !s.gateway.disabled().is_tool_disabled(&name) {
        return Json(s.gateway.disabled().snapshot()).into_response();
    }
    let nm = name.clone();
    mutate_and_rebuild(&s, "enable_tool", name, move |ds| ds.enable_tool(&nm)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn req(auth: Option<&str>) -> Request {
        let mut b = axum::http::Request::builder();
        if let Some(v) = auth {
            b = b.header(AUTHORIZATION, v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn presented_bearer_parses_scheme_case_insensitively_and_rejects_others() {
        for h in ["Bearer tok", "bearer tok", "BEARER tok"] {
            assert_eq!(presented_bearer(&req(Some(h))), Some("tok".to_string()));
        }
        assert_eq!(presented_bearer(&req(Some("Bearer "))), None); // empty token
        assert_eq!(presented_bearer(&req(Some("Basic tok"))), None);
        assert_eq!(presented_bearer(&req(None)), None);
    }

    #[test]
    fn authorize_maps_config_and_token_to_decision() {
        assert_eq!(authorize(None, Some("x")), AdminAuth::NotConfigured);
        assert_eq!(
            authorize(Some("sekret"), Some("sekret")),
            AdminAuth::Allowed
        );
        assert_eq!(authorize(Some("sekret"), Some("wrong")), AdminAuth::Denied);
        assert_eq!(authorize(Some("sekret"), None), AdminAuth::Denied);
    }
}
