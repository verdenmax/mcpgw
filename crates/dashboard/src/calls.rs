use observe::{CallRecord, CallSink};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// One call as exposed by the API: a single owned type covering BOTH the live ring (id = decimal
/// seq) and the audit-JSONL history replay (id = "h{ts}-{n}"). Owned (no `&'static`) so history
/// lines — which cannot deserialize into `CallRecord` — map into the same shape. Metadata only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallItem {
    pub id: String,
    pub ts_unix_ms: u64,
    pub meta_tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub latency_ms: u64,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    pub arg_bytes: usize,
    pub result_bytes: usize,
}

/// Filter for the calls list, applied identically to live and history items (operates on the
/// already-built `CallItem`, so the two data sources share one matcher). All `None` = match all.
#[derive(Debug, Default, Clone)]
pub struct CallFilter {
    pub meta_tool: Option<String>,
    pub upstream: Option<String>,
    pub target_tool: Option<String>,
    pub outcome: Option<String>,
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
}

impl CallFilter {
    pub fn matches(&self, c: &CallItem) -> bool {
        if let Some(m) = &self.meta_tool {
            if &c.meta_tool != m {
                return false;
            }
        }
        if let Some(u) = &self.upstream {
            if c.upstream.as_deref() != Some(u.as_str()) {
                return false;
            }
        }
        if let Some(t) = &self.target_tool {
            if c.target_tool.as_deref() != Some(t.as_str()) {
                return false;
            }
        }
        if let Some(o) = &self.outcome {
            if &c.outcome != o {
                return false;
            }
        }
        if let Some(s) = self.since_ms {
            if c.ts_unix_ms < s {
                return false;
            }
        }
        if let Some(u) = self.until_ms {
            if c.ts_unix_ms > u {
                return false;
            }
        }
        true
    }
}

/// Internal ring entry: a `CallRecord` plus a stable in-process seq used as its live id.
struct StoredCall {
    seq: u64,
    record: CallRecord,
}

impl StoredCall {
    fn to_item(&self) -> CallItem {
        let r = &self.record;
        CallItem {
            id: self.seq.to_string(),
            ts_unix_ms: r.ts_unix_ms,
            meta_tool: r.meta_tool.as_str().to_string(),
            target_tool: r.target_tool.clone(),
            upstream: r.upstream.clone(),
            latency_ms: r.latency_ms,
            outcome: r.outcome.as_str().to_string(),
            error_kind: r.error_kind.map(|s| s.to_string()),
            arg_bytes: r.arg_bytes,
            result_bytes: r.result_bytes,
        }
    }
}

/// Bounded in-memory ring of recent `CallRecord`s (newest-first on read), feeding the Calls
/// drill-down. Mirrors `DiscoveryRingSink`: full -> evict oldest. Each insert gets a monotonic seq
/// (stable id while still resident). Lock never spans `.await`.
pub struct CallRingSink {
    cap: usize,
    ring: Mutex<VecDeque<StoredCall>>,
    next_seq: AtomicU64,
}

impl CallRingSink {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            cap,
            ring: Mutex::new(VecDeque::with_capacity(cap.min(1024))),
            next_seq: AtomicU64::new(0),
        }
    }

    /// Newest-first page of items matching `filter`. Returns `(page, total_matched)` where `total`
    /// counts ALL matches (for pagination UIs), independent of `limit`/`offset`.
    pub fn query(
        &self,
        filter: &CallFilter,
        limit: usize,
        offset: usize,
    ) -> (Vec<CallItem>, usize) {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let matched: Vec<CallItem> = ring
            .iter()
            .rev()
            .map(|s| s.to_item())
            .filter(|c| filter.matches(c))
            .collect();
        let total = matched.len();
        let page = matched.into_iter().skip(offset).take(limit).collect();
        (page, total)
    }

    /// Resolve a live id (decimal seq) to its item, or `None` if evicted/never existed.
    pub fn get(&self, seq: u64) -> Option<CallItem> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().find(|s| s.seq == seq).map(|s| s.to_item())
    }
}

impl CallSink for CallRingSink {
    fn record(&self, rec: &CallRecord) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() == self.cap {
            ring.pop_front();
        }
        ring.push_back(StoredCall {
            seq,
            record: rec.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use observe::{CallOutcome, CallRecord, CallSink, MetaTool};

    fn rec(
        meta: MetaTool,
        upstream: Option<&str>,
        tool: Option<&str>,
        outcome: CallOutcome,
        ts: u64,
    ) -> CallRecord {
        CallRecord {
            ts_unix_ms: ts,
            meta_tool: meta,
            target_tool: tool.map(|s| s.to_string()),
            upstream: upstream.map(|s| s.to_string()),
            latency_ms: 1,
            outcome,
            error_kind: None,
            arg_bytes: 0,
            result_bytes: 0,
        }
    }

    #[test]
    fn ring_caps_and_returns_newest_first() {
        let ring = CallRingSink::new(2);
        ring.record(&rec(
            MetaTool::CallTool,
            Some("a"),
            Some("a__t"),
            CallOutcome::Ok,
            1,
        ));
        ring.record(&rec(
            MetaTool::CallTool,
            Some("b"),
            Some("b__t"),
            CallOutcome::Ok,
            2,
        ));
        ring.record(&rec(
            MetaTool::CallTool,
            Some("c"),
            Some("c__t"),
            CallOutcome::Ok,
            3,
        )); // evicts first
        let (items, total) = ring.query(&CallFilter::default(), 10, 0);
        assert_eq!(total, 2, "capacity 2");
        let ups: Vec<_> = items
            .iter()
            .map(|i| i.upstream.as_deref().unwrap())
            .collect();
        assert_eq!(ups, ["c", "b"], "newest first");
    }

    #[test]
    fn seq_is_monotonic_and_get_resolves_live_id() {
        let ring = CallRingSink::new(10);
        ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 1));
        ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 2));
        let (items, _) = ring.query(&CallFilter::default(), 10, 0);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].id, "0");
        let one = ring.get(1).expect("seq 1 present");
        assert_eq!(one.ts_unix_ms, 2);
        assert!(ring.get(999).is_none(), "absent seq -> None");
    }

    #[test]
    fn filter_by_each_dimension() {
        let ring = CallRingSink::new(10);
        ring.record(&rec(
            MetaTool::CallTool,
            Some("gh"),
            Some("gh__issue"),
            CallOutcome::Error,
            10,
        ));
        ring.record(&rec(
            MetaTool::SearchTools,
            Some("gh"),
            None,
            CallOutcome::Ok,
            20,
        ));
        ring.record(&rec(
            MetaTool::CallTool,
            Some("wx"),
            Some("wx__now"),
            CallOutcome::Ok,
            30,
        ));

        let f = CallFilter {
            meta_tool: Some("call_tool".into()),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 2);
        let f = CallFilter {
            upstream: Some("gh".into()),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 2);
        let f = CallFilter {
            target_tool: Some("wx__now".into()),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 1);
        let f = CallFilter {
            outcome: Some("error".into()),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 1);
        let f = CallFilter {
            since_ms: Some(20),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 2);
        let f = CallFilter {
            until_ms: Some(20),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 2);
    }

    #[test]
    fn pagination_offset_and_limit() {
        let ring = CallRingSink::new(10);
        for ts in 0..5 {
            ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, ts));
        }
        let (page, total) = ring.query(&CallFilter::default(), 2, 1);
        assert_eq!(total, 5, "total counts all matched, not just the page");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].ts_unix_ms, 3);
        assert_eq!(page[1].ts_unix_ms, 2);
    }

    #[test]
    fn empty_ring_and_limit_zero_and_offset_overflow() {
        let ring = CallRingSink::new(4);
        assert_eq!(ring.query(&CallFilter::default(), 10, 0), (vec![], 0));
        ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 1));
        assert_eq!(
            ring.query(&CallFilter::default(), 0, 0).0.len(),
            0,
            "limit 0 -> empty page"
        );
        assert_eq!(
            ring.query(&CallFilter::default(), 10, 99).0.len(),
            0,
            "offset past end -> empty page"
        );
        assert_eq!(
            ring.query(&CallFilter::default(), 10, 99).1,
            1,
            "...but total still 1"
        );
    }
}
