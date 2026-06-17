//! Read-only web dashboard for mcpgw (subsystem A): metrics aggregation, discovery traces,
//! and a static SPA served over a separate localhost port.

mod metrics;
pub use metrics::{MetaToolMetrics, MetricsSink, MetricsSnapshot, UpstreamMetrics};

mod trace;
pub use trace::{DiscoveryRingSink, DiscoveryWriter};

mod calls;
pub use calls::{CallFilter, CallItem, CallRingSink};

mod history;
pub use history::{replay_audit_calls, replay_audit_metrics, replay_discovery, MetricBucket};

mod api;
pub use api::{AppState, UpstreamInfo};

use axum::extract::Request;
use axum::extract::{Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::header::HOST;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse};
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
async fn h_tools(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Vec<api::ToolView>> {
    Json(api::tools(&s, q.get("q").map(|v| v.as_str())))
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

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLE_CSS: &str = include_str!("../assets/style.css");

async fn h_index() -> impl IntoResponse {
    Html(INDEX_HTML)
}
async fn h_app_js() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/javascript")], APP_JS)
}
async fn h_style_css() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/css")], STYLE_CSS)
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
        .route("/api/tools", get(h_tools))
        .route("/api/metrics", get(h_metrics))
        .route("/api/traces", get(h_traces))
        .route("/api/metrics/history", get(h_metrics_history))
        .route("/", get(h_index))
        .route("/app.js", get(h_app_js))
        .route("/style.css", get(h_style_css))
        .with_state(state);
    if enforce_loopback_host {
        router.layer(middleware::from_fn(require_local_host))
    } else {
        router
    }
}

#[cfg(test)]
mod asset_tests {
    use super::{APP_JS, INDEX_HTML, STYLE_CSS};

    #[test]
    fn embedded_assets_are_present_and_wired() {
        assert!(INDEX_HTML.contains("<title>"), "index has a title");
        assert!(INDEX_HTML.contains("app.js"), "index loads app.js");
        assert!(APP_JS.contains("/api/overview"), "app.js polls the API");
        assert!(STYLE_CSS.contains("{"), "style.css is non-empty CSS");
    }

    #[test]
    fn untrusted_trace_fields_are_html_escaped() {
        // The discovery trace renders client/upstream-controlled strings (query text and tool
        // names) into innerHTML, so they must go through escapeHtml to avoid stored XSS.
        assert!(
            APP_JS.contains("escapeHtml(r.query)"),
            "trace query is escaped"
        );
        assert!(
            APP_JS.contains("escapeHtml(h.name)"),
            "trace tool name is escaped"
        );
        assert!(
            APP_JS.contains("escapeHtml(u.reason)"),
            "upstream skip reason is escaped"
        );
        assert!(
            APP_JS.contains("escapeHtml(u.name)") && APP_JS.contains("escapeHtml(u.transport)"),
            "upstream name and transport are escaped"
        );
        assert!(
            APP_JS.contains("escapeHtml(x.meta_tool)"),
            "meta-tool name is escaped"
        );
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
