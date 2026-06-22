//! 活动聚合（只读、仅元数据）：把 live 调用环窗内的 `CallRecord` 元数据聚合为 dashboard 的
//! 趋势 sparkline（固定 24 桶）、error_kind 分布、最慢调用 / 最忙工具 Top-N。
//! 纯函数 `aggregate` 不依赖环内部结构，便于单测；隐私上 `ActivityResponse` 不含任何 args/result。

use serde::Serialize;
use std::collections::HashMap;

/// sparkline 固定桶数（柱数恒定，渲染稳定）。
pub const BUCKETS: usize = 24;
/// 排行榜 / 分布的 Top-N。
pub const TOP_N: usize = 5;

/// 从一条 ring 记录投影出的聚合输入（owned，使 `aggregate` 与环内部解耦、可独立单测）。
pub struct AggInput {
    pub id: String,
    pub ts_unix_ms: u64,
    pub meta_tool: String,
    pub target_tool: Option<String>,
    pub latency_ms: u64,
    pub outcome: String, // "ok" | "error" | "timeout"
    pub error_kind: Option<String>,
}

#[derive(Serialize, Default)]
pub struct ActivityResponse {
    pub window_ms: u64,
    pub bucket_ms: u64,
    pub buckets: Vec<ActivityBucket>,
    pub total: u64,
    pub errors: u64,
    pub by_error_kind: Vec<KindCount>,
    pub slowest: Vec<SlowCall>,
    pub busiest_tools: Vec<ToolCount>,
}

#[derive(Serialize, Default, Clone)]
pub struct ActivityBucket {
    pub t: u64,
    pub total: u64,
    pub errors: u64,
}

#[derive(Serialize)]
pub struct KindCount {
    pub kind: String,
    pub count: u64,
}

#[derive(Serialize)]
pub struct SlowCall {
    pub id: String,
    pub label: String,
    pub meta_tool: String,
    pub latency_ms: u64,
    pub outcome: String,
}

#[derive(Serialize)]
pub struct ToolCount {
    pub name: String,
    pub count: u64,
}

/// 聚合 `inputs` 中 `ts_unix_ms` 落在 `[now - bucket_ms*BUCKETS, now]` 窗内的调用。
/// `window_ms` 决定桶宽 `bucket_ms = (window_ms / BUCKETS).max(1)`；窗起点对齐到整 24 桶。
pub fn aggregate(inputs: &[AggInput], window_ms: u64, now: u64) -> ActivityResponse {
    let bucket_ms = (window_ms / BUCKETS as u64).max(1);
    let span = bucket_ms * BUCKETS as u64;
    let start = now.saturating_sub(span);

    let mut buckets: Vec<ActivityBucket> = (0..BUCKETS)
        .map(|i| ActivityBucket {
            t: start + i as u64 * bucket_ms,
            total: 0,
            errors: 0,
        })
        .collect();

    let mut total = 0u64;
    let mut errors = 0u64;
    let mut kind_map: HashMap<String, u64> = HashMap::new();
    let mut tool_map: HashMap<String, u64> = HashMap::new();
    // (latency, ts, id, label, meta_tool, outcome) 候选，最后排序取前 TOP_N。
    let mut slow: Vec<(u64, u64, String, String, String, String)> = Vec::new();

    for c in inputs {
        if c.ts_unix_ms < start {
            continue; // 窗外
        }
        // Clamp in u64 *before* the cast so a far-future ts can't wrap past 24 on a 32-bit usize.
        let idx = ((c.ts_unix_ms - start) / bucket_ms).min(BUCKETS as u64 - 1) as usize;
        let is_err = c.outcome != "ok";
        buckets[idx].total += 1;
        total += 1;
        if is_err {
            buckets[idx].errors += 1;
            errors += 1;
        }
        if let Some(k) = &c.error_kind {
            *kind_map.entry(k.clone()).or_insert(0) += 1;
        }
        if let Some(t) = &c.target_tool {
            *tool_map.entry(t.clone()).or_insert(0) += 1;
        }
        let label = c.target_tool.clone().unwrap_or_else(|| c.meta_tool.clone());
        slow.push((
            c.latency_ms,
            c.ts_unix_ms,
            c.id.clone(),
            label,
            c.meta_tool.clone(),
            c.outcome.clone(),
        ));
    }

    // by_error_kind：count 降序，并列 kind 名升序（稳定）。
    let mut by_error_kind: Vec<KindCount> = kind_map
        .into_iter()
        .map(|(kind, count)| KindCount { kind, count })
        .collect();
    by_error_kind.sort_by(|a, b| b.count.cmp(&a.count).then(a.kind.cmp(&b.kind)));

    // busiest_tools：count 降序，并列名升序，取前 TOP_N。
    let mut busiest_tools: Vec<ToolCount> = tool_map
        .into_iter()
        .map(|(name, count)| ToolCount { name, count })
        .collect();
    busiest_tools.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    busiest_tools.truncate(TOP_N);

    // slowest：latency 降序，并列按 ts 降序（更晚的在前），取前 TOP_N。
    slow.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let slowest: Vec<SlowCall> = slow
        .into_iter()
        .take(TOP_N)
        .map(
            |(latency_ms, _ts, id, label, meta_tool, outcome)| SlowCall {
                id,
                label,
                meta_tool,
                latency_ms,
                outcome,
            },
        )
        .collect();

    ActivityResponse {
        window_ms,
        bucket_ms,
        buckets,
        total,
        errors,
        by_error_kind,
        slowest,
        busiest_tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(
        ts: u64,
        latency: u64,
        outcome: &str,
        target: Option<&str>,
        kind: Option<&str>,
    ) -> AggInput {
        AggInput {
            id: format!("{ts}"),
            ts_unix_ms: ts,
            meta_tool: if target.is_some() {
                "call_tool".into()
            } else {
                "search_tools".into()
            },
            target_tool: target.map(|s| s.to_string()),
            latency_ms: latency,
            outcome: outcome.into(),
            error_kind: kind.map(|s| s.to_string()),
        }
    }

    #[test]
    fn buckets_are_fixed_count_and_width() {
        let now = 1_000_000;
        let r = aggregate(&[], 24_000, now);
        assert_eq!(r.buckets.len(), BUCKETS, "always 24 buckets");
        assert_eq!(r.bucket_ms, 1_000, "24000/24");
        assert_eq!(r.window_ms, 24_000);
        assert!(
            r.buckets.iter().all(|b| b.total == 0 && b.errors == 0),
            "empty -> all zero"
        );
        assert_eq!(r.total, 0);
    }

    #[test]
    fn out_of_window_excluded_and_now_in_last_bucket() {
        let now = 100_000;
        // window=24000 -> span=24000 -> start=76000；now-50_000=50_000 在窗外。
        let r = aggregate(
            &[
                inp(now - 50_000, 5, "ok", None, None),
                inp(now, 5, "ok", Some("a__x"), None),
            ],
            24_000,
            now,
        );
        assert_eq!(r.total, 1, "out-of-window call excluded");
        assert_eq!(
            r.buckets[BUCKETS - 1].total,
            1,
            "now falls in the last bucket"
        );
    }

    #[test]
    fn errors_counted_in_buckets_and_totals() {
        let now = 100_000;
        let r = aggregate(
            &[
                inp(now, 1, "ok", Some("a__x"), None),
                inp(now, 1, "error", Some("a__x"), Some("upstream_call")),
                inp(now, 1, "timeout", Some("a__y"), Some("timeout")),
            ],
            24_000,
            now,
        );
        assert_eq!(r.total, 3);
        assert_eq!(r.errors, 2, "error + timeout count as errors; ok does not");
        assert_eq!(r.buckets[BUCKETS - 1].errors, 2);
    }

    #[test]
    fn by_error_kind_only_non_ok_sorted_desc() {
        let now = 100_000;
        let r = aggregate(
            &[
                inp(now, 1, "error", Some("a"), Some("upstream_call")),
                inp(now, 1, "timeout", Some("a"), Some("timeout")),
                inp(now, 1, "error", Some("a"), Some("timeout")),
                inp(now, 1, "ok", Some("a"), None),
            ],
            24_000,
            now,
        );
        assert_eq!(r.by_error_kind.len(), 2);
        assert_eq!(r.by_error_kind[0].kind, "timeout");
        assert_eq!(r.by_error_kind[0].count, 2, "count desc");
        assert_eq!(r.by_error_kind[1].kind, "upstream_call");
    }

    #[test]
    fn busiest_only_counts_target_tool_and_truncates_top_n() {
        let now = 100_000;
        let mut v = vec![inp(now, 1, "ok", None, None)]; // search_tools -> no target -> excluded
        for (name, n) in [
            ("t1", 6),
            ("t2", 4),
            ("t3", 3),
            ("t4", 2),
            ("t5", 1),
            ("t6", 1),
        ] {
            for _ in 0..n {
                v.push(inp(now, 1, "ok", Some(name), None));
            }
        }
        let r = aggregate(&v, 24_000, now);
        assert_eq!(r.busiest_tools.len(), TOP_N, "top-5 only");
        assert_eq!(r.busiest_tools[0].name, "t1");
        assert_eq!(r.busiest_tools[0].count, 6);
        assert!(
            r.busiest_tools.iter().all(|t| t.name != "t6"),
            "6th tool dropped"
        );
    }

    #[test]
    fn slowest_is_latency_desc_top_n() {
        let now = 100_000;
        let v: Vec<AggInput> = [120u64, 50, 800, 300, 10, 999]
            .iter()
            .map(|&l| inp(now, l, "ok", Some("a__x"), None))
            .collect();
        let r = aggregate(&v, 24_000, now);
        assert_eq!(r.slowest.len(), TOP_N);
        assert_eq!(r.slowest[0].latency_ms, 999);
        assert_eq!(r.slowest[1].latency_ms, 800);
        assert_eq!(r.slowest[4].latency_ms, 50, "10ms dropped (6th)");
    }

    #[test]
    fn slow_label_falls_back_to_meta_tool() {
        let now = 100_000;
        let r = aggregate(&[inp(now, 5, "ok", None, None)], 24_000, now);
        assert_eq!(
            r.slowest[0].label, "search_tools",
            "label = meta_tool when no target"
        );
        assert_eq!(r.slowest[0].meta_tool, "search_tools");
    }

    #[test]
    fn response_has_no_payload_content_fields() {
        let now = 100_000;
        let r = aggregate(
            &[inp(now, 5, "error", Some("a__x"), Some("timeout"))],
            24_000,
            now,
        );
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"args\""), "no args field: {json}");
        assert!(!json.contains("\"result\""), "no result field: {json}");
    }
}
