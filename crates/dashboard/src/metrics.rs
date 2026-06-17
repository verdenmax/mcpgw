use observe::{CallOutcome, CallRecord, CallSink};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Fixed latency bucket upper bounds (ms); the last bucket is unbounded.
const BUCKETS_MS: [u64; 12] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 5000, u64::MAX];

#[derive(Default)]
struct MetaAgg {
    calls: u64,
    errors: u64,
    max_ms: u64,
    hist: [u64; BUCKETS_MS.len()],
}

impl MetaAgg {
    fn observe(&mut self, latency_ms: u64, is_error: bool) {
        self.calls += 1;
        if is_error {
            self.errors += 1;
        }
        self.max_ms = self.max_ms.max(latency_ms);
        let idx = BUCKETS_MS
            .iter()
            .position(|&b| latency_ms <= b)
            .unwrap_or(BUCKETS_MS.len() - 1);
        self.hist[idx] += 1;
    }

    /// Approximate percentile: the upper bound of the bucket where the cumulative count crosses
    /// `p` (0.0..=1.0), capped at the observed max so it never exceeds reality.
    fn percentile(&self, p: f64) -> u64 {
        if self.calls == 0 {
            return 0;
        }
        let target = (p * self.calls as f64).ceil() as u64;
        let mut cum = 0;
        for (i, &count) in self.hist.iter().enumerate() {
            cum += count;
            if cum >= target {
                return BUCKETS_MS[i].min(self.max_ms);
            }
        }
        self.max_ms
    }
}

#[derive(Default)]
struct OutcomeAgg {
    calls: u64,
    errors: u64,
}

#[derive(Default)]
struct MetricsState {
    total: u64,
    per_meta: BTreeMap<&'static str, MetaAgg>,
    per_upstream: BTreeMap<String, OutcomeAgg>,
}

/// Per-meta-tool metrics in a `MetricsSnapshot`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetaToolMetrics {
    pub meta_tool: String,
    pub calls: u64,
    pub errors: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

/// Per-upstream call metrics in a `MetricsSnapshot`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpstreamMetrics {
    pub upstream: String,
    pub calls: u64,
    pub errors: u64,
}

/// A point-in-time copy of the aggregated metrics, served by the dashboard API.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct MetricsSnapshot {
    pub total_calls: u64,
    pub per_meta_tool: Vec<MetaToolMetrics>,
    pub per_upstream: Vec<UpstreamMetrics>,
}

/// Cap on distinct per-upstream keys tracked. The `upstream` of a `CallRecord` is derived from a
/// client-supplied `call_tool` name (the `{prefix}__name` before resolution), so a client flooding
/// bogus names would otherwise grow `per_upstream` without bound. Real deployments have far fewer
/// upstreams than this, so the cap only ever bites under abuse.
const MAX_UPSTREAM_KEYS: usize = 1024;

/// In-memory aggregator of `CallRecord`s. Implements `observe::CallSink`; bounded — `per_meta` is
/// keyed on the finite meta-tool set and `per_upstream` is capped at `MAX_UPSTREAM_KEYS`. The lock
/// is a plain `Mutex` held only for the short aggregation/snapshot work.
pub struct MetricsSink {
    state: Mutex<MetricsState>,
}

impl MetricsSink {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MetricsState::default()),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        MetricsSnapshot {
            total_calls: st.total,
            per_meta_tool: st
                .per_meta
                .iter()
                .map(|(name, a)| MetaToolMetrics {
                    meta_tool: name.to_string(),
                    calls: a.calls,
                    errors: a.errors,
                    p50_ms: a.percentile(0.50),
                    p95_ms: a.percentile(0.95),
                    max_ms: a.max_ms,
                })
                .collect(),
            per_upstream: st
                .per_upstream
                .iter()
                .map(|(name, a)| UpstreamMetrics {
                    upstream: name.clone(),
                    calls: a.calls,
                    errors: a.errors,
                })
                .collect(),
        }
    }
}

impl Default for MetricsSink {
    fn default() -> Self {
        Self::new()
    }
}

impl CallSink for MetricsSink {
    fn record(&self, rec: &CallRecord) {
        // Any non-Ok outcome (Error or Timeout) counts as an error for the dashboard.
        let is_error = !matches!(rec.outcome, CallOutcome::Ok);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.total += 1;
        st.per_meta
            .entry(rec.meta_tool.as_str())
            .or_default()
            .observe(rec.latency_ms, is_error);
        if let Some(up) = &rec.upstream {
            // Update an existing key, or insert a new one only while under the cap, so a flood of
            // distinct (possibly client-controlled) upstream names can't grow the map without bound.
            let map = &mut st.per_upstream;
            if map.contains_key(up.as_str()) || map.len() < MAX_UPSTREAM_KEYS {
                let agg = map.entry(up.clone()).or_default();
                agg.calls += 1;
                if is_error {
                    agg.errors += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use observe::MetaTool;

    fn rec(
        meta: MetaTool,
        outcome: CallOutcome,
        latency: u64,
        upstream: Option<&str>,
    ) -> CallRecord {
        CallRecord {
            ts_unix_ms: 0,
            meta_tool: meta,
            target_tool: None,
            upstream: upstream.map(|s| s.to_string()),
            latency_ms: latency,
            outcome,
            error_kind: None,
            arg_bytes: 0,
            result_bytes: 0,
        }
    }

    #[test]
    fn aggregates_counts_errors_and_latency() {
        let sink = MetricsSink::new();
        sink.record(&rec(
            MetaTool::CallTool,
            CallOutcome::Ok,
            10,
            Some("github"),
        ));
        sink.record(&rec(
            MetaTool::CallTool,
            CallOutcome::Error,
            30,
            Some("github"),
        ));
        sink.record(&rec(MetaTool::SearchTools, CallOutcome::Ok, 5, None));
        let snap = sink.snapshot();
        assert_eq!(snap.total_calls, 3);
        let ct = snap
            .per_meta_tool
            .iter()
            .find(|m| m.meta_tool == "call_tool")
            .unwrap();
        assert_eq!(ct.calls, 2);
        assert_eq!(ct.errors, 1);
        assert_eq!(ct.max_ms, 30);
        let gh = snap
            .per_upstream
            .iter()
            .find(|u| u.upstream == "github")
            .unwrap();
        assert_eq!(gh.calls, 2);
        assert_eq!(gh.errors, 1);
    }

    #[test]
    fn percentiles_are_monotonic_and_bounded_by_max() {
        let sink = MetricsSink::new();
        for ms in [1u64, 2, 5, 10, 50, 100, 200, 400] {
            sink.record(&rec(MetaTool::SearchTools, CallOutcome::Ok, ms, None));
        }
        let snap = sink.snapshot();
        let s = snap
            .per_meta_tool
            .iter()
            .find(|m| m.meta_tool == "search_tools")
            .unwrap();
        assert!(s.p50_ms <= s.p95_ms, "p50 <= p95");
        assert!(s.p95_ms <= s.max_ms, "p95 <= max");
        assert_eq!(s.max_ms, 400);
    }

    #[test]
    fn timeout_outcome_counts_as_an_error() {
        let sink = MetricsSink::new();
        sink.record(&rec(
            MetaTool::CallTool,
            CallOutcome::Timeout,
            50,
            Some("github"),
        ));
        let snap = sink.snapshot();
        let ct = snap
            .per_meta_tool
            .iter()
            .find(|m| m.meta_tool == "call_tool")
            .unwrap();
        assert_eq!(ct.calls, 1);
        assert_eq!(ct.errors, 1, "a Timeout outcome is an error");
        let gh = snap
            .per_upstream
            .iter()
            .find(|u| u.upstream == "github")
            .unwrap();
        assert_eq!(gh.errors, 1);
    }

    #[test]
    fn per_upstream_keys_are_capped() {
        let sink = MetricsSink::new();
        for i in 0..(MAX_UPSTREAM_KEYS + 50) {
            sink.record(&rec(
                MetaTool::CallTool,
                CallOutcome::Ok,
                1,
                Some(&format!("u{i}")),
            ));
        }
        let snap = sink.snapshot();
        assert_eq!(
            snap.per_upstream.len(),
            MAX_UPSTREAM_KEYS,
            "distinct upstream keys are bounded"
        );
        assert_eq!(
            snap.total_calls,
            (MAX_UPSTREAM_KEYS + 50) as u64,
            "all calls are still counted even past the upstream cap"
        );
    }

    #[test]
    fn empty_snapshot_is_zeroed() {
        let snap = MetricsSink::new().snapshot();
        assert_eq!(snap.total_calls, 0);
        assert!(snap.per_meta_tool.is_empty());
        assert!(snap.per_upstream.is_empty());
    }
}
