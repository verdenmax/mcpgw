//! Read-only web dashboard for mcpgw (subsystem A): metrics aggregation, discovery traces,
//! and a static SPA served over a separate localhost port.

mod metrics;
pub use metrics::{MetaToolMetrics, MetricsSink, MetricsSnapshot, UpstreamMetrics};

mod trace;
pub use trace::{DiscoveryRingSink, DiscoveryWriter};

mod history;
pub use history::{replay_audit_metrics, replay_discovery, MetricBucket};

mod api;
pub use api::{AppState, UpstreamInfo};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::Json;
use std::collections::HashMap;
use std::sync::Arc;

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
    let limit = qparam_usize(&q, "limit", 100);
    let source = q.get("source").cloned().unwrap_or_else(|| "live".into());
    Json(api::traces(&s, limit, &source))
}
async fn h_metrics_history(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<api::HistoryResponse> {
    let limit = qparam_usize(&q, "limit", 5000);
    let bucket_ms = q
        .get("bucket_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000u64);
    Json(api::metrics_history(&s, limit, bucket_ms))
}

/// Build the dashboard's API router (static assets added in Task 10).
pub fn build_dashboard_router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route("/api/overview", get(h_overview))
        .route("/api/upstreams", get(h_upstreams))
        .route("/api/tools", get(h_tools))
        .route("/api/metrics", get(h_metrics))
        .route("/api/traces", get(h_traces))
        .route("/api/metrics/history", get(h_metrics_history))
        .with_state(state)
}
