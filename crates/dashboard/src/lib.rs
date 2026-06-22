//! Read-only web dashboard for mcpgw (subsystem A): metrics aggregation, discovery traces,
//! and a static SPA served over a separate localhost port.

mod metrics;
pub use metrics::{MetaToolMetrics, MetricsSink, MetricsSnapshot, UpstreamMetrics};

mod trace;
pub use trace::{DiscoveryRingSink, DiscoveryWriter, TraceItem};

mod calls;
pub use calls::{CallFilter, CallItem, CallRingSink};

// `pub use` is added in Task 2; until then the module is private, so allow the
// items to be unreachable from the crate root without tripping `-D warnings`.
#[allow(dead_code)]
mod activity;

mod history;
pub use history::{replay_audit_calls, replay_audit_metrics, replay_discovery_items, MetricBucket};

mod api;
pub use api::{AppState, UpstreamInfo};

mod assets;

use axum::extract::Request;
use axum::extract::{Path, Query, State};
use axum::http::header::HOST;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Json;
use std::collections::HashMap;
use std::sync::Arc;

/// Upper clamp on a client-supplied `limit` for history replay, so a hostile/accidental huge
/// value can't make `tail_lines` buffer an unbounded number of lines from a large JSONL file.
const MAX_HISTORY_LIMIT: usize = 50_000;

fn qparam_usize(q: &HashMap<String, String>, key: &str, default: usize) -> usize {
    q.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

async fn h_overview(State(s): State<Arc<AppState>>) -> Json<api::Overview> {
    Json(api::overview(&s))
}
async fn h_upstreams(State(s): State<Arc<AppState>>) -> Json<Vec<api::UpstreamView>> {
    Json(api::upstreams(&s))
}
async fn h_upstream_detail(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> axum::response::Response {
    match api::upstream_detail(&s, &name) {
        Some(d) => Json(d).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn h_tools(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Vec<api::ToolView>> {
    Json(api::tools(&s, q.get("q").map(|v| v.as_str())))
}
async fn h_tool_detail(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> axum::response::Response {
    match api::tool_detail(&s, &name) {
        Some(d) => Json(d).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn h_metrics(State(s): State<Arc<AppState>>) -> Json<MetricsSnapshot> {
    Json(api::metrics(&s))
}
async fn h_traces(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<api::TracesResponse> {
    let limit = qparam_usize(&q, "limit", 100).min(MAX_HISTORY_LIMIT);
    let source = q.get("source").cloned().unwrap_or_else(|| "live".into());
    // The live path reads the in-memory ring (fast); the history path reads a JSONL file, so run it
    // on the blocking pool to avoid stalling a runtime worker that also serves live MCP traffic.
    if source == "history" {
        let resp = tokio::task::spawn_blocking(move || api::traces(&s, limit, &source))
            .await
            .expect("traces history replay task");
        Json(resp)
    } else {
        Json(api::traces(&s, limit, &source))
    }
}
async fn h_metrics_history(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<api::HistoryResponse> {
    let limit = qparam_usize(&q, "limit", 5000).min(MAX_HISTORY_LIMIT);
    let bucket_ms = q
        .get("bucket_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000u64);
    // Reads the audit JSONL from disk; offload to the blocking pool (see h_traces).
    let resp = tokio::task::spawn_blocking(move || api::metrics_history(&s, limit, bucket_ms))
        .await
        .expect("audit metrics history replay task");
    Json(resp)
}

async fn h_calls(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<api::CallsResponse> {
    let filter = api::call_filter_from_query(&q);
    let source = q.get("source").cloned().unwrap_or_else(|| "live".into());
    let limit = qparam_usize(&q, "limit", 100).min(MAX_HISTORY_LIMIT);
    let offset = qparam_usize(&q, "offset", 0);
    // History reads a JSONL file off the blocking pool (see h_traces); live reads the in-memory ring.
    if source == "history" {
        let resp = tokio::task::spawn_blocking(move || {
            api::calls(&s, &filter, &source, api::CALL_HISTORY_SCAN, limit, offset)
        })
        .await
        .expect("calls history replay task");
        Json(resp)
    } else {
        Json(api::calls(
            &s,
            &filter,
            &source,
            api::CALL_HISTORY_SCAN,
            limit,
            offset,
        ))
    }
}

async fn h_call_detail(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if api::is_history_id(&id) {
        let detail = tokio::task::spawn_blocking(move || api::call_detail(&s, &id))
            .await
            .expect("call detail replay task");
        match detail {
            Some(item) => Json(item).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        match api::call_detail(&s, &id) {
            Some(item) => Json(item).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

async fn h_trace_detail(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if api::is_history_id(&id) {
        let detail = tokio::task::spawn_blocking(move || api::trace_detail(&s, &id))
            .await
            .expect("trace detail replay task");
        match detail {
            Some(t) => Json(t).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        match api::trace_detail(&s, &id) {
            Some(t) => Json(t).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

/// True if the `Host` header names the local machine (the literal `localhost`, or an IP that is a
/// loopback address). Defends the unauthenticated dashboard against DNS rebinding when bound to
/// loopback: a remote page that rebinds its hostname to 127.0.0.1 still sends its OWN hostname in
/// `Host`, which is rejected. Missing/unparseable Host -> not local.
fn host_is_local(host: Option<&str>) -> bool {
    let Some(raw) = host else {
        return false;
    };
    // A valid `Host` never contains userinfo ('@'); reject it defensively so a smuggled
    // `localhost:80@evil.com`-style authority can't be mistaken for a local host.
    if raw.contains('@') {
        return false;
    }
    // Strip the optional port. IPv6 hosts are bracketed in `Host`: `[::1]:8971` / `[::1]`.
    let host = if let Some(rest) = raw.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, _)) => inner,
            None => return false,
        }
    } else {
        raw.rsplit_once(':').map_or(raw, |(h, _)| h)
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Reject (403) any request whose `Host` is not local. Mounted only when the dashboard is bound to
/// loopback (see `build_dashboard_router`).
async fn require_local_host(req: Request, next: Next) -> axum::response::Response {
    let host = req.headers().get(HOST).and_then(|v| v.to_str().ok());
    if host_is_local(host) {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Build the dashboard's router. When `enforce_loopback_host` is true (dashboard bound to
/// loopback), a layer rejects requests whose `Host` isn't local, closing the DNS-rebinding vector.
pub fn build_dashboard_router(state: Arc<AppState>, enforce_loopback_host: bool) -> axum::Router {
    let router = axum::Router::new()
        .route("/api/overview", get(h_overview))
        .route("/api/upstreams", get(h_upstreams))
        .route("/api/upstreams/{name}", get(h_upstream_detail))
        .route("/api/tools", get(h_tools))
        .route("/api/tools/{name}", get(h_tool_detail))
        .route("/api/metrics", get(h_metrics))
        .route("/api/traces", get(h_traces))
        .route("/api/traces/{id}", get(h_trace_detail))
        .route("/api/metrics/history", get(h_metrics_history))
        .route("/api/calls", get(h_calls))
        .route("/api/calls/{id}", get(h_call_detail))
        .fallback(assets::static_handler)
        .with_state(state);
    if enforce_loopback_host {
        router.layer(middleware::from_fn(require_local_host))
    } else {
        router
    }
}

#[cfg(test)]
mod host_tests {
    use super::host_is_local;

    #[test]
    fn host_is_local_accepts_loopback_rejects_remote() {
        for ok in [
            "127.0.0.1:8971",
            "127.0.0.1",
            "localhost",
            "localhost:8971",
            "LOCALHOST:8971",
            "[::1]:8971",
            "[::1]",
            "127.0.0.5:8971",
        ] {
            assert!(host_is_local(Some(ok)), "{ok} should be local");
        }
        for bad in [
            "evil.com:8971",
            "192.168.1.5:8971",
            "example.com",
            "0.0.0.0:8971",
            "[::]:8971",
            "localhost:80@evil.com",
            "127.0.0.1@evil.com",
        ] {
            assert!(!host_is_local(Some(bad)), "{bad} should NOT be local");
        }
        assert!(!host_is_local(None), "missing Host -> not local");
    }
}
