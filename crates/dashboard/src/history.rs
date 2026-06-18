use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::calls::{CallFilter, CallItem};
use crate::trace::TraceItem;

/// Read the last `limit` newline-delimited lines of a file, keeping at most `limit` lines in
/// memory at once (so a large file with well-formed JSONL — one bounded record per line — uses
/// bounded memory). Oldest-first; `None` if the file can't be opened. A line-read error
/// (non-UTF-8 / IO) ends the tail early, yielding the lines read so far (the JSONL we write is
/// always valid UTF-8, so this only matters for externally-corrupted files).
fn tail_lines(path: &Path, limit: usize) -> Option<Vec<String>> {
    let file = std::fs::File::open(path).ok()?;
    let limit = limit.max(1);
    let mut ring: VecDeque<String> = VecDeque::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if ring.len() == limit {
            ring.pop_front();
        }
        ring.push_back(line);
    }
    Some(ring.into_iter().collect())
}

/// Replay the discovery JSONL into `TraceItem`s, newest-first, scanning at most the last `limit`
/// lines; bad lines skipped. Each item gets a stable id `"h{ts}-{n}"` (n counts same-ts in file
/// order). `DiscoveryRecord` derives `Deserialize`, so no owned-mirror is needed. Bool = readable.
pub fn replay_discovery_items(path: &Path, limit: usize) -> (Vec<TraceItem>, bool) {
    let Some(lines) = tail_lines(path, limit) else {
        return (Vec::new(), false);
    };
    let mut ts_counts: BTreeMap<u64, u32> = BTreeMap::new();
    let mut items: Vec<TraceItem> = Vec::new();
    for line in &lines {
        if let Ok(r) = serde_json::from_str::<observe::DiscoveryRecord>(line) {
            let n = ts_counts.entry(r.ts_unix_ms).or_insert(0);
            let id = format!("h{}-{}", r.ts_unix_ms, *n);
            *n += 1;
            items.push(TraceItem {
                id,
                ts_unix_ms: r.ts_unix_ms,
                query: r.query,
                top_k: r.top_k,
                results: r.results,
                latency_ms: r.latency_ms,
            });
        }
    }
    items.reverse();
    (items, true)
}

/// One fixed-width time bucket of audit metrics.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MetricBucket {
    pub bucket_start_ms: u64,
    pub calls: u64,
    pub errors: u64,
}

#[derive(Deserialize)]
struct AuditLine {
    ts_unix_ms: u64,
    outcome: String,
}

/// Replay the audit JSONL into fixed-width time buckets (`bucket_ms`), oldest-first, scanning at
/// most the last `limit` lines. Bad lines are skipped. Bool = file present/readable.
pub fn replay_audit_metrics(
    path: &Path,
    limit: usize,
    bucket_ms: u64,
) -> (Vec<MetricBucket>, bool) {
    let Some(lines) = tail_lines(path, limit) else {
        return (Vec::new(), false);
    };
    let bucket_ms = bucket_ms.max(1);
    let mut buckets: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for line in &lines {
        if let Ok(a) = serde_json::from_str::<AuditLine>(line) {
            let start = a.ts_unix_ms - (a.ts_unix_ms % bucket_ms);
            let e = buckets.entry(start).or_insert((0, 0));
            e.0 += 1;
            // Any non-"ok" outcome (e.g. "error" or "timeout") counts as an error, matching the
            // live MetricsSink so the live and historical error counts stay consistent.
            if a.outcome != "ok" {
                e.1 += 1;
            }
        }
    }
    let out = buckets
        .into_iter()
        .map(|(start, (calls, errors))| MetricBucket {
            bucket_start_ms: start,
            calls,
            errors,
        })
        .collect();
    (out, true)
}

/// One audit JSONL line as an OWNED mirror of `CallRecord` (which derives only `Serialize` and
/// holds `error_kind: &'static str`, so it can't be deserialized directly). Absent optional fields
/// default to `None`/`0`.
#[derive(Deserialize)]
struct AuditCallLine {
    ts_unix_ms: u64,
    meta_tool: String,
    #[serde(default)]
    target_tool: Option<String>,
    #[serde(default)]
    upstream: Option<String>,
    latency_ms: u64,
    outcome: String,
    #[serde(default)]
    error_kind: Option<String>,
    #[serde(default)]
    arg_bytes: usize,
    #[serde(default)]
    result_bytes: usize,
}

/// Replay the audit JSONL into `CallItem`s, newest-first, scanning at most the last `scan_limit`
/// lines. Bad lines are skipped. Each item gets a stable id `"h{ts}-{n}"` where `n` counts
/// same-`ts` records in file order (stable for an unchanged file tail). `filter` is applied after
/// id assignment so ids don't shift with the filter. Bool = file present/readable.
pub fn replay_audit_calls(
    path: &Path,
    scan_limit: usize,
    filter: &CallFilter,
) -> (Vec<CallItem>, bool) {
    let Some(lines) = tail_lines(path, scan_limit) else {
        return (Vec::new(), false);
    };
    let mut ts_counts: BTreeMap<u64, u32> = BTreeMap::new();
    let mut items: Vec<CallItem> = Vec::new();
    for line in &lines {
        if let Ok(a) = serde_json::from_str::<AuditCallLine>(line) {
            let n = ts_counts.entry(a.ts_unix_ms).or_insert(0);
            let id = format!("h{}-{}", a.ts_unix_ms, *n);
            *n += 1;
            let item = CallItem {
                id,
                ts_unix_ms: a.ts_unix_ms,
                meta_tool: a.meta_tool,
                target_tool: a.target_tool,
                upstream: a.upstream,
                latency_ms: a.latency_ms,
                outcome: a.outcome,
                error_kind: a.error_kind,
                arg_bytes: a.arg_bytes,
                result_bytes: a.result_bytes,
                args: None,
                args_truncated: false,
                result: None,
                result_truncated: false,
            };
            if filter.matches(&item) {
                items.push(item);
            }
        }
    }
    items.reverse(); // file order is oldest-first -> newest-first
    (items, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, body: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("mcpgw-hist-{}-{name}", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn replay_discovery_missing_file_is_unavailable() {
        let p = std::env::temp_dir().join("mcpgw-hist-does-not-exist.jsonl");
        let _ = std::fs::remove_file(&p);
        let (items, ok) = replay_discovery_items(&p, 10);
        assert!(items.is_empty());
        assert!(!ok);
    }

    #[test]
    fn replay_discovery_skips_bad_lines_newest_first_with_stable_ids() {
        let body = "{\"ts_unix_ms\":1,\"query\":\"a\",\"top_k\":1,\"results\":[],\"latency_ms\":0}\n\
                    not json\n\
                    {\"ts_unix_ms\":1,\"query\":\"b\",\"top_k\":1,\"results\":[],\"latency_ms\":0}\n\
                    {\"ts_unix_ms\":2,\"query\":\"c\",\"top_k\":1,\"results\":[],\"latency_ms\":0}\n";
        let p = write("disc.jsonl", body);
        let (items, ok) = replay_discovery_items(&p, 10);
        assert!(ok);
        let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["h2-0", "h1-1", "h1-0"],
            "newest first, bad line skipped, ids stable: n counts same-ts in file order"
        );
        let qs: Vec<_> = items.iter().map(|i| i.query.as_str()).collect();
        assert_eq!(qs, ["c", "b", "a"], "newest first");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn replay_audit_buckets_calls_and_errors() {
        let body = "{\"ts_unix_ms\":0,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":10,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"error\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":1000,\"meta_tool\":\"search_tools\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = write("audit.jsonl", body);
        let (buckets, ok) = replay_audit_metrics(&p, 100, 1000);
        assert!(ok);
        assert_eq!(buckets.len(), 2);
        assert_eq!(
            buckets[0],
            MetricBucket {
                bucket_start_ms: 0,
                calls: 2,
                errors: 1
            }
        );
        assert_eq!(
            buckets[1],
            MetricBucket {
                bucket_start_ms: 1000,
                calls: 1,
                errors: 0
            }
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn replay_audit_counts_timeout_as_an_error() {
        let body = "{\"ts_unix_ms\":0,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"timeout\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = write("audit-timeout.jsonl", body);
        let (buckets, ok) = replay_audit_metrics(&p, 100, 1000);
        assert!(ok);
        assert_eq!(buckets.len(), 1);
        assert_eq!(
            buckets[0],
            MetricBucket {
                bucket_start_ms: 0,
                calls: 2,
                errors: 1
            },
            "a timeout outcome counts as an error, matching the live metrics"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn replay_audit_calls_missing_file_is_unavailable() {
        let p = std::env::temp_dir().join("mcpgw-calls-does-not-exist.jsonl");
        let _ = std::fs::remove_file(&p);
        let (items, ok) = replay_audit_calls(&p, 10, &crate::calls::CallFilter::default());
        assert!(items.is_empty());
        assert!(!ok);
    }

    #[test]
    fn replay_audit_calls_skips_bad_lines_newest_first_with_stable_ids() {
        let body = "{\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"upstream\":\"gh\",\"target_tool\":\"gh__a\",\"latency_ms\":2,\"outcome\":\"ok\",\"arg_bytes\":3,\"result_bytes\":4}\n\
                    not json\n\
                    {\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"error\",\"error_kind\":\"upstream\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":2,\"meta_tool\":\"search_tools\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = write("calls.jsonl", body);
        let (items, ok) = replay_audit_calls(&p, 10, &crate::calls::CallFilter::default());
        assert!(ok);
        let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["h2-0", "h1-1", "h1-0"],
            "ids stable: n counts same-ts in file order"
        );
        assert_eq!(items[0].meta_tool, "search_tools");
        assert_eq!(items[2].upstream.as_deref(), Some("gh"));
        assert_eq!(items[1].error_kind.as_deref(), Some("upstream"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn replay_audit_calls_applies_filter() {
        let body = "{\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"upstream\":\"gh\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":2,\"meta_tool\":\"call_tool\",\"upstream\":\"wx\",\"latency_ms\":1,\"outcome\":\"error\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = write("calls-filter.jsonl", body);
        let f = crate::calls::CallFilter {
            outcome: Some("error".into()),
            ..Default::default()
        };
        let (items, ok) = replay_audit_calls(&p, 10, &f);
        assert!(ok);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].upstream.as_deref(), Some("wx"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn replay_audit_calls_ids_are_stable_regardless_of_filter() {
        let body = "{\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":1,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"error\",\"arg_bytes\":0,\"result_bytes\":0}\n\
                    {\"ts_unix_ms\":2,\"meta_tool\":\"call_tool\",\"latency_ms\":1,\"outcome\":\"error\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = write("calls-stableids.jsonl", body);
        // Unfiltered: the error rows get ids h1-1 and h2-0.
        let (all, _) = replay_audit_calls(&p, 10, &crate::calls::CallFilter::default());
        let h1_1 = all
            .iter()
            .find(|i| i.id == "h1-1")
            .expect("h1-1 present unfiltered");
        assert_eq!(h1_1.outcome, "error");
        // Filtered to outcome=error: the SAME surviving rows keep the SAME ids (filter doesn't shift n).
        let f = crate::calls::CallFilter {
            outcome: Some("error".into()),
            ..Default::default()
        };
        let (errs, _) = replay_audit_calls(&p, 10, &f);
        let ids: Vec<_> = errs.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["h2-0", "h1-1"],
            "ids identical with or without the filter"
        );
        let _ = std::fs::remove_file(&p);
    }
}
