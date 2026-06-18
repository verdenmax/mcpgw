use observe::{CallContent, CallContentSink, CallRecord};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub args_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub result_truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Case-insensitive substring of `needle` over `args` + (optional) `result`.
fn content_contains(args: &str, result: Option<&str>, needle: &str) -> bool {
    let n = needle.to_lowercase();
    args.to_lowercase().contains(&n)
        || result
            .map(|r| r.to_lowercase().contains(&n))
            .unwrap_or(false)
}

/// Parse `args` JSON and recursively check for a key `k` (exact, case-sensitive) whose stringified
/// value contains `v` (case-insensitive). Truncated/invalid JSON -> no match (best-effort).
fn args_key_value_matches(args: &str, k: &str, v: &str) -> bool {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(args) else {
        return false;
    };
    let needle = v.to_lowercase();
    fn walk(val: &serde_json::Value, k: &str, needle: &str) -> bool {
        match val {
            serde_json::Value::Object(m) => m.iter().any(|(key, child)| {
                let hit_here = key == k && {
                    let s = match child {
                        serde_json::Value::String(s) => s.to_lowercase(),
                        other => other.to_string().to_lowercase(),
                    };
                    s.contains(needle)
                };
                hit_here || walk(child, k, needle)
            }),
            serde_json::Value::Array(a) => a.iter().any(|x| walk(x, k, needle)),
            _ => false,
        }
    }
    walk(&val, k, &needle)
}

/// Filter for the calls list, applied identically to live and history items (operates on the
/// already-built `CallItem`, so the two data sources share one matcher). All `None` = match all.
/// `since_ms` and `until_ms` are BOTH inclusive: the time window is the closed interval `[since, until]`.
#[derive(Debug, Default, Clone)]
pub struct CallFilter {
    pub meta_tool: Option<String>,
    pub upstream: Option<String>,
    pub target_tool: Option<String>,
    pub outcome: Option<String>,
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub q: Option<String>,
    pub arg_key: Option<String>,
    pub arg_val: Option<String>,
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
        // Content filters apply ONLY to items carrying content (live ring built with content);
        // history / list-light items have `args == None` and are NOT excluded by content filters.
        if let Some(args) = &c.args {
            if let Some(q) = &self.q {
                if !content_contains(args, c.result.as_deref(), q) {
                    return false;
                }
            }
            if let (Some(k), Some(v)) = (&self.arg_key, &self.arg_val) {
                if !args_key_value_matches(args, k, v) {
                    return false;
                }
            }
        }
        true
    }
}

/// Internal ring entry: a `CallRecord` plus a stable in-process seq used as its live id.
struct StoredCall {
    seq: u64,
    record: CallRecord,
    content: CallContent,
}

impl StoredCall {
    fn to_item(&self, with_content: bool) -> CallItem {
        let r = &self.record;
        let (args, args_truncated, result, result_truncated) = if with_content {
            let c = &self.content;
            (
                Some(c.args.clone()),
                c.args_truncated,
                Some(c.result.clone()),
                c.result_truncated,
            )
        } else {
            (None, false, None, false)
        };
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
            args,
            args_truncated,
            result,
            result_truncated,
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
            // Cap the pre-reservation: the call ring default is 2000, so avoid reserving slots that
            // may never fill; pay at most one re-grow if the ring does exceed 1024 live entries.
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
        // Build content only when a content filter is active (q / arg pair); otherwise keep the
        // light, allocation-free path for ordinary list polling.
        let want_content =
            filter.q.is_some() || (filter.arg_key.is_some() && filter.arg_val.is_some());
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        // Building a CallItem per ring entry (so the single CallFilter::matches(&CallItem) matcher is
        // reused across the live ring and JSONL history) is a deliberate readability/reuse tradeoff,
        // fine for the bounded ring.
        let matched: Vec<CallItem> = ring
            .iter()
            .rev()
            .filter_map(|s| {
                // First pass on a light item: matches() checks metadata only (content filters are
                // gated by Some(args), which a light item lacks). Build full content + re-check
                // (now applying content filters) ONLY for metadata survivors.
                let light = s.to_item(false);
                if !filter.matches(&light) {
                    return None;
                }
                if !want_content {
                    return Some(light);
                }
                let full = s.to_item(true);
                filter.matches(&full).then_some(full)
            })
            .collect();
        drop(ring); // pagination math below doesn't need the lock; release it off the record() hot path
        let total = matched.len();
        let mut page: Vec<CallItem> = matched.into_iter().skip(offset).take(limit).collect();
        if want_content {
            // The list never returns content; we built it only to filter, now strip it.
            for c in &mut page {
                c.args = None;
                c.args_truncated = false;
                c.result = None;
                c.result_truncated = false;
            }
        }
        (page, total)
    }

    /// Resolve a live id (decimal seq) to its item, or `None` if evicted/never existed.
    pub fn get(&self, seq: u64) -> Option<CallItem> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().find(|s| s.seq == seq).map(|s| s.to_item(true))
    }
}

impl CallContentSink for CallRingSink {
    fn record(&self, meta: &CallRecord, content: &CallContent) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        // Allocate the seq while holding the lock so physical ring order always matches seq order
        // (the fan-out calls record() concurrently); otherwise a race could push out of seq order.
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        if ring.len() == self.cap {
            ring.pop_front();
        }
        ring.push_back(StoredCall {
            seq,
            record: meta.clone(),
            content: content.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use observe::{CallOutcome, CallRecord, MetaTool};

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

    fn content() -> observe::CallContent {
        observe::CallContent {
            args: "{\"text\":\"hi\"}".into(),
            args_truncated: false,
            result: "{\"ok\":true}".into(),
            result_truncated: false,
        }
    }

    fn content_of(args: &str, result: &str) -> observe::CallContent {
        observe::CallContent {
            args: args.into(),
            args_truncated: false,
            result: result.into(),
            result_truncated: false,
        }
    }

    #[test]
    fn ring_stores_content_detail_includes_list_omits() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content(),
        );
        let (items, _) = ring.query(&CallFilter::default(), 10, 0);
        assert!(items[0].args.is_none(), "list omits args");
        assert!(items[0].result.is_none(), "list omits result");
        let d = ring.get(0).expect("seq 0");
        assert_eq!(d.args.as_deref(), Some("{\"text\":\"hi\"}"));
        assert_eq!(d.result.as_deref(), Some("{\"ok\":true}"));
        assert!(!d.args_truncated);
    }

    #[test]
    fn ring_caps_and_returns_newest_first() {
        let ring = CallRingSink::new(2);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("a"),
                Some("a__t"),
                CallOutcome::Ok,
                1,
            ),
            &content(),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("b"),
                Some("b__t"),
                CallOutcome::Ok,
                2,
            ),
            &content(),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("c"),
                Some("c__t"),
                CallOutcome::Ok,
                3,
            ),
            &content(),
        ); // evicts first
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
        ring.record(
            &rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 1),
            &content(),
        );
        ring.record(
            &rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 2),
            &content(),
        );
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
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__issue"),
                CallOutcome::Error,
                10,
            ),
            &content(),
        );
        ring.record(
            &rec(MetaTool::SearchTools, Some("gh"), None, CallOutcome::Ok, 20),
            &content(),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("wx"),
                Some("wx__now"),
                CallOutcome::Ok,
                30,
            ),
            &content(),
        );

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
            ring.record(
                &rec(MetaTool::CallTool, None, None, CallOutcome::Ok, ts),
                &content(),
            );
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
        ring.record(
            &rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 1),
            &content(),
        );
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

    #[test]
    fn combined_filters_are_anded() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content(),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("wx"),
                Some("wx__b"),
                CallOutcome::Error,
                2,
            ),
            &content(),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__c"),
                CallOutcome::Error,
                3,
            ),
            &content(),
        );
        let f = CallFilter {
            upstream: Some("gh".into()),
            outcome: Some("error".into()),
            ..Default::default()
        };
        let (items, total) = ring.query(&f, 10, 0);
        assert_eq!(total, 1, "AND of upstream=gh and outcome=error");
        assert_eq!(items[0].target_tool.as_deref(), Some("gh__c"));
    }

    #[test]
    fn error_kind_round_trips_into_item() {
        let ring = CallRingSink::new(10);
        let mut r = rec(
            MetaTool::CallTool,
            Some("gh"),
            None,
            CallOutcome::Timeout,
            1,
        );
        r.error_kind = Some("timeout");
        ring.record(&r, &content());
        let (items, _) = ring.query(&CallFilter::default(), 10, 0);
        assert_eq!(items[0].error_kind.as_deref(), Some("timeout"));
        assert_eq!(items[0].outcome, "timeout");
    }

    #[test]
    fn query_free_text_filters_over_args_and_result() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content_of("{\"text\":\"hello\"}", "{\"ok\":1}"),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__b"),
                CallOutcome::Ok,
                2,
            ),
            &content_of("{\"text\":\"world\"}", "{\"ok\":2}"),
        );
        let f = CallFilter {
            q: Some("hello".into()),
            ..Default::default()
        };
        let (items, total) = ring.query(&f, 10, 0);
        assert_eq!(total, 1, "free-text matches args content");
        assert!(
            items[0].args.is_none(),
            "list omits args after content filter"
        );
    }

    #[test]
    fn query_arg_key_value_recurses_nested_args() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content_of("{\"name\":\"gh__a\",\"arguments\":{\"text\":\"hi\"}}", "{}"),
        );
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__b"),
                CallOutcome::Ok,
                2,
            ),
            &content_of(
                "{\"name\":\"gh__b\",\"arguments\":{\"text\":\"bye\"}}",
                "{}",
            ),
        );
        let f = CallFilter {
            arg_key: Some("text".into()),
            arg_val: Some("hi".into()),
            ..Default::default()
        };
        assert_eq!(
            ring.query(&f, 10, 0).1,
            1,
            "arg_key=text arg_val=hi matches nested"
        );
    }

    #[test]
    fn content_filters_skip_items_without_content() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(MetaTool::CallTool, Some("gh"), None, CallOutcome::Ok, 1),
            &content_of("{}", "{}"),
        );
        let f = CallFilter {
            meta_tool: Some("call_tool".into()),
            ..Default::default()
        };
        assert_eq!(ring.query(&f, 10, 0).1, 1);
    }

    #[test]
    fn query_free_text_matches_result_only() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content_of("{\"text\":\"x\"}", "{\"echoed\":\"needle42\"}"),
        );
        let f = CallFilter {
            q: Some("needle42".into()),
            ..Default::default()
        };
        assert_eq!(
            ring.query(&f, 10, 0).1,
            1,
            "free-text matches result content (not just args)"
        );
    }

    #[test]
    fn arg_filter_invalid_json_does_not_match_or_panic() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content_of("{not valid json", "{}"),
        );
        let f = CallFilter {
            arg_key: Some("text".into()),
            arg_val: Some("hi".into()),
            ..Default::default()
        };
        assert_eq!(
            ring.query(&f, 10, 0).1,
            0,
            "invalid args JSON -> no match, no panic"
        );
    }

    #[test]
    fn arg_filter_matches_numeric_value() {
        let ring = CallRingSink::new(10);
        ring.record(
            &rec(
                MetaTool::CallTool,
                Some("gh"),
                Some("gh__a"),
                CallOutcome::Ok,
                1,
            ),
            &content_of("{\"n\":42}", "{}"),
        );
        let f = CallFilter {
            arg_key: Some("n".into()),
            arg_val: Some("42".into()),
            ..Default::default()
        };
        assert_eq!(
            ring.query(&f, 10, 0).1,
            1,
            "numeric value stringified and matched"
        );
    }
}
