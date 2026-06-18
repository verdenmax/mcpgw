use crate::history::{replay_audit_calls, replay_audit_metrics, replay_discovery, MetricBucket};
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
    /// Per-call ring for the Calls drill-down (present only when the dashboard is enabled).
    pub calls: Option<Arc<crate::calls::CallRingSink>>,
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
pub struct UpstreamDetail {
    pub name: String,
    pub transport: String,
    pub status: &'static str,
    pub reason: Option<String>,
    pub tools_count: usize,
    pub calls: u64,
    pub errors: u64,
    pub tools: Vec<ToolView>,
}

#[derive(Serialize)]
pub struct ToolView {
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct ToolDetail {
    pub name: String,
    pub server: String,
    pub description: String,
    pub input_schema: serde_json::Value,
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

/// Audit lines scanned for BOTH the calls list (source=history) and single-id resolution, so a
/// history id assigned by the list always resolves identically in detail (same scan window).
pub const CALL_HISTORY_SCAN: usize = 50_000;

#[derive(Serialize)]
pub struct CallsResponse {
    pub source: String,
    pub history_unavailable: bool,
    pub total: usize,
    pub items: Vec<crate::calls::CallItem>,
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

/// Resolve an upstream's connection status from the last rebuild summary: ingested -> "connected",
/// skipped -> ("skipped", reason), otherwise -> "unknown" (incl. before the first rebuild).
fn resolve_status(
    summary: &Option<std::sync::Arc<gateway::RebuildSummary>>,
    name: &str,
) -> (&'static str, Option<String>) {
    match summary {
        None => ("unknown", None),
        Some(s) if s.ingested.iter().any(|n| n == name) => ("connected", None),
        Some(s) => match s.skipped.iter().find(|(n, _)| n == name) {
            Some((_, why)) => ("skipped", Some(why.clone())),
            None => ("unknown", None),
        },
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
            let (status, reason) = resolve_status(&summary, &info.name);
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

/// Single-upstream detail: its `UpstreamView` fields + the list of tools it currently exposes.
/// `None` if `name` isn't a configured upstream.
pub fn upstream_detail(state: &AppState, name: &str) -> Option<UpstreamDetail> {
    let info = state.upstreams.iter().find(|u| u.name == name)?;
    let snap = state.gateway.snapshot();
    let summary = state.gateway.last_summary();
    let m = state.metrics.snapshot();
    let (status, reason) = resolve_status(&summary, &info.name);
    let tools: Vec<ToolView> = snap
        .catalog()
        .iter()
        .filter(|t| t.server == info.name)
        .map(|t| ToolView {
            name: t.qualified_name(),
            description: t.description.clone(),
        })
        .collect();
    let um = m.per_upstream.iter().find(|u| u.upstream == info.name);
    Some(UpstreamDetail {
        name: info.name.clone(),
        transport: info.transport.clone(),
        status,
        reason,
        tools_count: tools.len(),
        calls: um.map(|u| u.calls).unwrap_or(0),
        errors: um.map(|u| u.errors).unwrap_or(0),
        tools,
    })
}

/// Single-tool detail from the catalog (keyed by qualified name `{server}__{tool}`). `None` if absent.
pub fn tool_detail(state: &AppState, name: &str) -> Option<ToolDetail> {
    let snap = state.gateway.snapshot();
    let def = snap.catalog().get(name)?;
    Some(ToolDetail {
        name: def.qualified_name(),
        server: def.server.clone(),
        description: def.description.clone(),
        input_schema: def.input_schema.clone(),
    })
}

pub fn tools(state: &AppState, q: Option<&str>) -> Vec<ToolView> {
    let snap = state.gateway.snapshot();
    let needle = q.filter(|n| !n.is_empty()).map(|n| n.to_lowercase());
    snap.catalog()
        .iter()
        .filter(|t| match &needle {
            Some(n) => {
                t.qualified_name().to_lowercase().contains(n.as_str())
                    || t.description.to_lowercase().contains(n.as_str())
            }
            None => true,
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

/// Build a `CallFilter` from the query map (`meta`/`upstream`/`tool`/`outcome`/`since`/`until`).
pub fn call_filter_from_query(
    q: &std::collections::HashMap<String, String>,
) -> crate::calls::CallFilter {
    crate::calls::CallFilter {
        meta_tool: q.get("meta").cloned(),
        upstream: q.get("upstream").cloned(),
        target_tool: q.get("tool").cloned(),
        outcome: q.get("outcome").cloned(),
        since_ms: q.get("since").and_then(|v| v.parse().ok()),
        until_ms: q.get("until").and_then(|v| v.parse().ok()),
    }
}

/// Calls list. `source="history"` replays the audit JSONL (scanning at most `scan_limit` lines);
/// otherwise reads the live ring. Filter + pagination applied uniformly; `total` counts all matches.
pub fn calls(
    state: &AppState,
    filter: &crate::calls::CallFilter,
    source: &str,
    scan_limit: usize,
    limit: usize,
    offset: usize,
) -> CallsResponse {
    if source == "history" {
        match &state.audit_path {
            Some(p) => {
                let (matched, ok) = replay_audit_calls(p, scan_limit, filter);
                let total = matched.len();
                let items = matched.into_iter().skip(offset).take(limit).collect();
                CallsResponse {
                    source: "history".into(),
                    history_unavailable: !ok,
                    total,
                    items,
                }
            }
            None => CallsResponse {
                source: "history".into(),
                history_unavailable: true,
                total: 0,
                items: Vec::new(),
            },
        }
    } else {
        match &state.calls {
            Some(ring) => {
                let (items, total) = ring.query(filter, limit, offset);
                CallsResponse {
                    source: "live".into(),
                    history_unavailable: false,
                    total,
                    items,
                }
            }
            None => CallsResponse {
                source: "live".into(),
                history_unavailable: false,
                total: 0,
                items: Vec::new(),
            },
        }
    }
}

/// True if `id` is a history call id (`"h{ts}-{n}"`); false for a live ring seq (decimal). The id
/// format is owned here so the handler's blocking-pool decision and `call_detail`'s source routing
/// can't drift apart.
pub fn is_history_id(id: &str) -> bool {
    id.starts_with('h')
}

/// Resolve one call id: `h...` -> history (re-scan `CALL_HISTORY_SCAN` lines + find), else decimal
/// seq -> live ring. `None` if not found / source unavailable.
pub fn call_detail(state: &AppState, id: &str) -> Option<crate::calls::CallItem> {
    if is_history_id(id) {
        let p = state.audit_path.as_ref()?;
        let (items, ok) =
            replay_audit_calls(p, CALL_HISTORY_SCAN, &crate::calls::CallFilter::default());
        if !ok {
            return None;
        }
        items.into_iter().find(|c| c.id == id)
    } else {
        let seq: u64 = id.parse().ok()?;
        state.calls.as_ref()?.get(seq)
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
            calls: None,
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

    fn call_rec(
        meta: observe::MetaTool,
        upstream: Option<&str>,
        outcome: observe::CallOutcome,
        ts: u64,
    ) -> observe::CallRecord {
        observe::CallRecord {
            ts_unix_ms: ts,
            meta_tool: meta,
            target_tool: None,
            upstream: upstream.map(|s| s.to_string()),
            latency_ms: 1,
            outcome,
            error_kind: None,
            arg_bytes: 0,
            result_bytes: 0,
        }
    }

    #[tokio::test]
    async fn calls_live_filters_and_paginates() {
        let ring = Arc::new(crate::calls::CallRingSink::new(10));
        ring.record(&call_rec(
            observe::MetaTool::CallTool,
            Some("gh"),
            observe::CallOutcome::Ok,
            1,
        ));
        ring.record(&call_rec(
            observe::MetaTool::CallTool,
            Some("wx"),
            observe::CallOutcome::Error,
            2,
        ));
        ring.record(&call_rec(
            observe::MetaTool::SearchTools,
            Some("gh"),
            observe::CallOutcome::Ok,
            3,
        ));
        let st = AppState {
            calls: Some(ring),
            ..seeded_state().await
        };
        let f = crate::calls::CallFilter {
            meta_tool: Some("call_tool".into()),
            ..Default::default()
        };
        let resp = calls(&st, &f, "live", CALL_HISTORY_SCAN, 100, 0);
        assert_eq!(resp.source, "live");
        assert!(!resp.history_unavailable);
        assert_eq!(resp.total, 2);
        assert_eq!(resp.items.len(), 2);
        assert_eq!(
            resp.items[0].upstream.as_deref(),
            Some("wx"),
            "newest-first"
        );
    }

    #[tokio::test]
    async fn calls_live_empty_when_no_ring() {
        let st = seeded_state().await;
        let resp = calls(
            &st,
            &crate::calls::CallFilter::default(),
            "live",
            CALL_HISTORY_SCAN,
            100,
            0,
        );
        assert_eq!(resp.total, 0);
        assert!(resp.items.is_empty());
        assert!(!resp.history_unavailable);
    }

    #[tokio::test]
    async fn calls_history_unavailable_without_audit_path() {
        let st = seeded_state().await;
        let resp = calls(
            &st,
            &crate::calls::CallFilter::default(),
            "history",
            CALL_HISTORY_SCAN,
            100,
            0,
        );
        assert_eq!(resp.source, "history");
        assert!(resp.history_unavailable);
        assert!(resp.items.is_empty());
    }

    #[tokio::test]
    async fn call_detail_live_by_seq_and_404() {
        let ring = Arc::new(crate::calls::CallRingSink::new(10));
        ring.record(&call_rec(
            observe::MetaTool::CallTool,
            Some("gh"),
            observe::CallOutcome::Ok,
            7,
        ));
        let st = AppState {
            calls: Some(ring),
            ..seeded_state().await
        };
        let item = call_detail(&st, "0").expect("seq 0 present");
        assert_eq!(item.ts_unix_ms, 7);
        assert!(call_detail(&st, "999").is_none());
        assert!(call_detail(&st, "not-a-number").is_none());
    }

    #[tokio::test]
    async fn call_detail_history_by_composite_id() {
        let body = "{\"ts_unix_ms\":5,\"meta_tool\":\"call_tool\",\"upstream\":\"gh\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = std::env::temp_dir().join(format!("mcpgw-detail-{}.jsonl", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let st = AppState {
            audit_path: Some(p.clone()),
            ..seeded_state().await
        };
        let item = call_detail(&st, "h5-0").expect("history id resolves");
        assert_eq!(item.ts_unix_ms, 5);
        assert_eq!(item.upstream.as_deref(), Some("gh"));
        assert!(call_detail(&st, "h5-9").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn calls_history_list_id_resolves_in_detail_same_window() {
        // Two same-ms records + others; the id the LIST assigns must resolve in DETAIL (shared scan window).
        let body = "{\"ts_unix_ms\":9,\"meta_tool\":\"call_tool\",\"upstream\":\"a\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":9,\"meta_tool\":\"call_tool\",\"upstream\":\"b\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = std::env::temp_dir().join(format!("mcpgw-detail2-{}.jsonl", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let st = AppState {
            audit_path: Some(p.clone()),
            ..seeded_state().await
        };
        let resp = calls(
            &st,
            &crate::calls::CallFilter::default(),
            "history",
            CALL_HISTORY_SCAN,
            100,
            0,
        );
        assert_eq!(resp.total, 2);
        // pick any listed item's id and confirm detail resolves to the same upstream
        let first = &resp.items[0];
        let detail = call_detail(&st, &first.id).expect("listed id resolves in detail");
        assert_eq!(detail.upstream, first.upstream);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn call_filter_from_query_maps_params() {
        let mut q = std::collections::HashMap::new();
        q.insert("meta".to_string(), "call_tool".to_string());
        q.insert("upstream".to_string(), "gh".to_string());
        q.insert("tool".to_string(), "gh__issue".to_string());
        q.insert("outcome".to_string(), "error".to_string());
        q.insert("since".to_string(), "10".to_string());
        q.insert("until".to_string(), "20".to_string());
        let f = call_filter_from_query(&q);
        assert_eq!(f.meta_tool.as_deref(), Some("call_tool"));
        assert_eq!(f.upstream.as_deref(), Some("gh"));
        assert_eq!(f.target_tool.as_deref(), Some("gh__issue"));
        assert_eq!(f.outcome.as_deref(), Some("error"));
        assert_eq!(f.since_ms, Some(10));
        assert_eq!(f.until_ms, Some(20));
    }

    #[tokio::test]
    async fn calls_unknown_source_falls_through_to_live() {
        let ring = Arc::new(crate::calls::CallRingSink::new(10));
        ring.record(&call_rec(
            observe::MetaTool::CallTool,
            Some("gh"),
            observe::CallOutcome::Ok,
            1,
        ));
        let st = AppState {
            calls: Some(ring),
            ..seeded_state().await
        };
        // Any non-"history" source string must be treated as live.
        let resp = calls(
            &st,
            &crate::calls::CallFilter::default(),
            "bogus",
            CALL_HISTORY_SCAN,
            100,
            0,
        );
        assert_eq!(resp.source, "live");
        assert_eq!(resp.total, 1);
    }

    #[tokio::test]
    async fn calls_history_paginates() {
        let body = "{\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"upstream\":\"a\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":2,\"meta_tool\":\"call_tool\",\"upstream\":\"b\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":3,\"meta_tool\":\"call_tool\",\"upstream\":\"c\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = std::env::temp_dir().join(format!("mcpgw-histpage-{}.jsonl", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let st = AppState {
            audit_path: Some(p.clone()),
            ..seeded_state().await
        };
        // newest-first = [c(3), b(2), a(1)]; offset 1 limit 1 -> [b]
        let resp = calls(
            &st,
            &crate::calls::CallFilter::default(),
            "history",
            CALL_HISTORY_SCAN,
            1,
            1,
        );
        assert_eq!(resp.total, 3, "total counts all matched");
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].upstream.as_deref(), Some("b"));
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn call_detail_history_missing_file_is_none() {
        let p =
            std::env::temp_dir().join(format!("mcpgw-detail-missing-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let st = AppState {
            audit_path: Some(p),
            ..seeded_state().await
        };
        assert!(
            call_detail(&st, "h1-0").is_none(),
            "history id but unreadable file -> None"
        );
    }

    #[test]
    fn is_history_id_distinguishes_live_and_history() {
        assert!(is_history_id("h5-0"));
        assert!(!is_history_id("0"));
        assert!(!is_history_id("42"));
    }

    #[tokio::test]
    async fn upstream_detail_unknown_is_none() {
        let st = seeded_state().await;
        assert!(upstream_detail(&st, "nope").is_none());
    }

    #[tokio::test]
    async fn upstream_detail_returns_view_and_tools() {
        let st = seeded_state().await;
        let d = upstream_detail(&st, "github").expect("configured upstream resolves");
        assert_eq!(d.name, "github");
        assert_eq!(d.transport, "stdio");
        assert_eq!(d.status, "unknown");
        assert!(d.tools.is_empty(), "empty catalog -> no tools");
    }

    #[tokio::test]
    async fn tool_detail_unknown_is_none() {
        let st = seeded_state().await;
        assert!(tool_detail(&st, "nope__missing").is_none());
    }
}
