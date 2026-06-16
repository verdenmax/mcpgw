use observe::DiscoveryRecord;
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Read up to the last `limit` lines of a file (memory bounded to `limit` lines regardless of
/// file size). Oldest-first; `None` if the file can't be opened.
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

/// Replay the discovery JSONL: newest-first, scanning at most the last `limit` lines. Bad lines are
/// skipped. Bool = file present/readable.
pub fn replay_discovery(path: &Path, limit: usize) -> (Vec<DiscoveryRecord>, bool) {
    let Some(lines) = tail_lines(path, limit) else {
        return (Vec::new(), false);
    };
    let mut recs: Vec<DiscoveryRecord> = lines
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    recs.reverse();
    (recs, true)
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
        let (recs, ok) = replay_discovery(&p, 10);
        assert!(recs.is_empty());
        assert!(!ok);
    }

    #[test]
    fn replay_discovery_skips_bad_lines_and_is_newest_first() {
        let body = "{\"ts_unix_ms\":1,\"query\":\"a\",\"top_k\":1,\"results\":[],\"latency_ms\":0}\n\
                    not json\n\
                    {\"ts_unix_ms\":2,\"query\":\"b\",\"top_k\":1,\"results\":[],\"latency_ms\":0}\n";
        let p = write("disc.jsonl", body);
        let (recs, ok) = replay_discovery(&p, 10);
        assert!(ok);
        let qs: Vec<_> = recs.iter().map(|r| r.query.as_str()).collect();
        assert_eq!(qs, ["b", "a"], "newest first, bad line skipped");
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
}
