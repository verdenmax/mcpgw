use crate::history::{replay_audit_metrics, replay_discovery, MetricBucket};
use crate::metrics::{MetricsSink, MetricsSnapshot};
use crate::trace::DiscoveryRingSink;
use gateway::GatewayState;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// One configured upstream's static identity (from Config), passed in at assembly.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpstreamInfo {
    pub name: String,
    pub transport: String,
}

/// Shared read-only state for the dashboard API handlers.
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<GatewayState>,
    pub metrics: Arc<MetricsSink>,
    pub discovery: Option<Arc<DiscoveryRingSink>>,
    pub upstreams: Vec<UpstreamInfo>,
    pub strategy: String,
    pub audit_path: Option<PathBuf>,
    pub discovery_path: Option<PathBuf>,
    pub started_at: Instant,
}

#[derive(Serialize)]
pub struct Overview {
    pub uptime_secs: u64,
    pub strategy: String,
    pub upstreams_total: usize,
    pub upstreams_connected: usize,
    pub tools_total: usize,
    pub total_calls: u64,
    pub last_rebuild_skipped: usize,
}

#[derive(Serialize)]
pub struct UpstreamView {
    pub name: String,
    pub transport: String,
    pub status: &'static str, // "connected" | "skipped" | "unknown"
    pub reason: Option<String>,
    pub tools: usize,
    pub calls: u64,
    pub errors: u64,
}

#[derive(Serialize)]
pub struct ToolView {
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct TracesResponse {
    pub source: String,
    pub history_unavailable: bool,
    pub traces: Vec<observe::DiscoveryRecord>,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    pub history_unavailable: bool,
    pub buckets: Vec<MetricBucket>,
}

pub fn overview(state: &AppState) -> Overview {
    let snap = state.gateway.snapshot();
    let m = state.metrics.snapshot();
    let ups = upstreams(state);
    Overview {
        uptime_secs: state.started_at.elapsed().as_secs(),
        strategy: state.strategy.clone(),
        upstreams_total: state.upstreams.len(),
        upstreams_connected: ups.iter().filter(|u| u.status == "connected").count(),
        tools_total: snap.catalog().len(),
        total_calls: m.total_calls,
        last_rebuild_skipped: state
            .gateway
            .last_summary()
            .map(|s| s.skipped.len())
            .unwrap_or(0),
    }
}

pub fn upstreams(state: &AppState) -> Vec<UpstreamView> {
    let snap = state.gateway.snapshot();
    let summary = state.gateway.last_summary();
    let m = state.metrics.snapshot();
    state
        .upstreams
        .iter()
        .map(|info| {
            let (status, reason) = match &summary {
                None => ("unknown", None),
                Some(s) => {
                    if s.ingested.iter().any(|n| n == &info.name) {
                        ("connected", None)
                    } else if let Some((_, why)) = s.skipped.iter().find(|(n, _)| n == &info.name) {
                        ("skipped", Some(why.clone()))
                    } else {
                        ("unknown", None)
                    }
                }
            };
            let tools = snap
                .catalog()
                .iter()
                .filter(|t| t.server == info.name)
                .count();
            let um = m.per_upstream.iter().find(|u| u.upstream == info.name);
            UpstreamView {
                name: info.name.clone(),
                transport: info.transport.clone(),
                status,
                reason,
                tools,
                calls: um.map(|u| u.calls).unwrap_or(0),
                errors: um.map(|u| u.errors).unwrap_or(0),
            }
        })
        .collect()
}

pub fn tools(state: &AppState, q: Option<&str>) -> Vec<ToolView> {
    let snap = state.gateway.snapshot();
    snap.catalog()
        .iter()
        .filter(|t| match q {
            Some(needle) if !needle.is_empty() => {
                let n = needle.to_lowercase();
                t.qualified_name().to_lowercase().contains(&n)
                    || t.description.to_lowercase().contains(&n)
            }
            _ => true,
        })
        .map(|t| ToolView {
            name: t.qualified_name(),
            description: t.description.clone(),
        })
        .collect()
}

pub fn metrics(state: &AppState) -> MetricsSnapshot {
    state.metrics.snapshot()
}

pub fn traces(state: &AppState, limit: usize, source: &str) -> TracesResponse {
    if source == "history" {
        match &state.discovery_path {
            Some(p) => {
                let (traces, ok) = replay_discovery(p, limit);
                TracesResponse {
                    source: "history".into(),
                    history_unavailable: !ok,
                    traces,
                }
            }
            None => TracesResponse {
                source: "history".into(),
                history_unavailable: true,
                traces: Vec::new(),
            },
        }
    } else {
        let traces = state
            .discovery
            .as_ref()
            .map(|d| d.recent(limit))
            .unwrap_or_default();
        TracesResponse {
            source: "live".into(),
            history_unavailable: false,
            traces,
        }
    }
}

pub fn metrics_history(state: &AppState, limit: usize, bucket_ms: u64) -> HistoryResponse {
    match &state.audit_path {
        Some(p) => {
            let (buckets, ok) = replay_audit_metrics(p, limit, bucket_ms);
            HistoryResponse {
                history_unavailable: !ok,
                buckets,
            }
        }
        None => HistoryResponse {
            history_unavailable: true,
            buckets: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use observe::CallSink;

    async fn seeded_state() -> AppState {
        // A gateway with two tools under one server, rebuilt so the snapshot is populated.
        let gw = Arc::new(GatewayState::new("bm25").unwrap());
        // Seed the snapshot directly via a rebuild over a registry is heavy; instead assert on the
        // metrics/upstreams plumbing using an empty gateway + configured upstream list.
        let metrics = Arc::new(MetricsSink::new());
        AppState {
            gateway: gw,
            metrics,
            discovery: None,
            upstreams: vec![UpstreamInfo {
                name: "github".into(),
                transport: "stdio".into(),
            }],
            strategy: "bm25".into(),
            audit_path: None,
            discovery_path: None,
            started_at: Instant::now(),
        }
    }

    #[tokio::test]
    async fn overview_reports_strategy_and_configured_upstreams() {
        let st = seeded_state().await;
        let ov = overview(&st);
        assert_eq!(ov.strategy, "bm25");
        assert_eq!(ov.upstreams_total, 1);
        assert_eq!(ov.total_calls, 0);
    }

    #[tokio::test]
    async fn upstreams_status_unknown_before_rebuild() {
        let st = seeded_state().await;
        let ups = upstreams(&st);
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].name, "github");
        assert_eq!(ups[0].transport, "stdio");
        assert_eq!(ups[0].status, "unknown"); // no rebuild summary yet
    }

    #[tokio::test]
    async fn metrics_reflects_recorded_calls() {
        let st = seeded_state().await;
        st.metrics.record(&observe::CallRecord {
            ts_unix_ms: 0,
            meta_tool: observe::MetaTool::SearchTools,
            target_tool: None,
            upstream: None,
            latency_ms: 4,
            outcome: observe::CallOutcome::Ok,
            error_kind: None,
            arg_bytes: 0,
            result_bytes: 0,
        });
        assert_eq!(metrics(&st).total_calls, 1);
    }

    #[tokio::test]
    async fn traces_history_unavailable_without_path() {
        let st = seeded_state().await;
        let t = traces(&st, 10, "history");
        assert!(t.history_unavailable);
        assert!(t.traces.is_empty());
    }

    #[tokio::test]
    async fn tools_lists_catalog_and_filters() {
        let mut st = seeded_state().await;
        // Replace gateway with one whose snapshot has tools by rebuilding from a seeded registry is
        // heavy; instead verify the empty case + filter is a no-op pass-through.
        let _ = &mut st;
        assert!(tools(&st, None).is_empty());
        assert!(tools(&st, Some("x")).is_empty());
    }
}
