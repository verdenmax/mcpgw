# Dashboard 活动聚合洞察（Activity Insights）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在只读 dashboard 上新增后端聚合端点 `/api/activity` 并据此渲染趋势 sparkline、时间范围过滤、error_kind 列+分布、最慢/最忙排行榜——全程只读、仅元数据。

**Architecture:** dashboard crate 新增纯函数聚合模块 `activity.rs`（`ActivityResponse` 类型 + `aggregate(&[AggInput], window_ms, now)`）；`CallRingSink::activity` 在锁内把窗内调用投影成 `AggInput` 交给 `aggregate`；`api::activity` + `h_activity` 路由暴露 `GET /api/activity?window=`。前端新增 `Sparkline.svelte`（纯内联 SVG）+ `Activity.svelte`（按中央 `refresh.tick` 拉取并渲染），接入 Overview 与 Calls。

**Tech Stack:** Rust（axum + serde, dashboard crate）、Svelte 5 runes + Vite（rust-embed 内嵌 dist）。

参考 spec：`docs/superpowers/specs/2026-06-22-mcpgw-dashboard-activity-insights-design.md`

---

## 文件结构

- **Create** `crates/dashboard/src/activity.rs` —— `ActivityResponse`/`ActivityBucket`/`KindCount`/`SlowCall`/`ToolCount`/`AggInput` 类型 + 纯函数 `aggregate` + 单测。一个职责：聚合算法。
- **Modify** `crates/dashboard/src/calls.rs` —— 加 `CallRingSink::activity(window_ms) -> ActivityResponse`（锁内投影 + 调 `aggregate`）。
- **Modify** `crates/dashboard/src/api.rs` —— 加 `pub fn activity(&AppState, window_ms) -> ActivityResponse`。
- **Modify** `crates/dashboard/src/lib.rs` —— `mod activity; pub use ...;` + `h_activity` handler + 路由 `/api/activity`。
- **Create** `crates/dashboard/ui/src/lib/Sparkline.svelte`、`crates/dashboard/ui/src/lib/Activity.svelte`。
- **Modify** `crates/dashboard/ui/src/lib/Overview.svelte`、`crates/dashboard/ui/src/lib/Calls.svelte`、`crates/dashboard/ui/src/app.css`。
- **Modify** `crates/mcpgw/tests/dashboard.rs` —— mock 上游 e2e 加 `/api/activity` 断言。
- **Modify** `docs/L1-overview.md`、`docs/L2-components/dashboard.md`、`docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`。

---

## Task 1：后端聚合 —— `activity.rs` 类型 + `aggregate` 纯函数

**Files:**
- Create: `crates/dashboard/src/activity.rs`
- Modify: `crates/dashboard/src/lib.rs`（仅加 `mod activity;` 让其编译 + 跑测）

- [ ] **Step 1: 在 `lib.rs` 注册模块**

在 `crates/dashboard/src/lib.rs` 的 `mod calls;` 之后加一行（仅为让新文件参与编译/测试；`pub use` 留到 Task 2）：

```rust
mod activity;
```

- [ ] **Step 2: 写 `activity.rs` 的类型与 `aggregate` 骨架 + 失败测试**

创建 `crates/dashboard/src/activity.rs`：

```rust
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
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(ts: u64, latency: u64, outcome: &str, target: Option<&str>, kind: Option<&str>) -> AggInput {
        AggInput {
            id: format!("{ts}"),
            ts_unix_ms: ts,
            meta_tool: if target.is_some() { "call_tool".into() } else { "search_tools".into() },
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
        assert!(r.buckets.iter().all(|b| b.total == 0 && b.errors == 0), "empty -> all zero");
        assert_eq!(r.total, 0);
    }
}
```

- [ ] **Step 3: 跑测确认失败（`unimplemented!`）**

Run: `cargo test -p dashboard activity:: 2>&1 | tail -20`
Expected: `buckets_are_fixed_count_and_width` panics（`not implemented`）→ FAIL。

- [ ] **Step 4: 实现 `aggregate`**

把 `activity.rs` 里的 `pub fn aggregate(...) { unimplemented!() }` 整体替换为：

```rust
pub fn aggregate(inputs: &[AggInput], window_ms: u64, now: u64) -> ActivityResponse {
    let bucket_ms = (window_ms / BUCKETS as u64).max(1);
    let span = bucket_ms * BUCKETS as u64;
    let start = now.saturating_sub(span);

    let mut buckets: Vec<ActivityBucket> = (0..BUCKETS)
        .map(|i| ActivityBucket { t: start + i as u64 * bucket_ms, total: 0, errors: 0 })
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
        let idx = (((c.ts_unix_ms - start) / bucket_ms) as usize).min(BUCKETS - 1);
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
    let mut by_error_kind: Vec<KindCount> =
        kind_map.into_iter().map(|(kind, count)| KindCount { kind, count }).collect();
    by_error_kind.sort_by(|a, b| b.count.cmp(&a.count).then(a.kind.cmp(&b.kind)));

    // busiest_tools：count 降序，并列名升序，取前 TOP_N。
    let mut busiest_tools: Vec<ToolCount> =
        tool_map.into_iter().map(|(name, count)| ToolCount { name, count }).collect();
    busiest_tools.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    busiest_tools.truncate(TOP_N);

    // slowest：latency 降序，并列按 ts 降序（更晚的在前），取前 TOP_N。
    slow.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let slowest: Vec<SlowCall> = slow
        .into_iter()
        .take(TOP_N)
        .map(|(latency_ms, _ts, id, label, meta_tool, outcome)| SlowCall {
            id,
            label,
            meta_tool,
            latency_ms,
            outcome,
        })
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
```

- [ ] **Step 5: 跑测确认通过**

Run: `cargo test -p dashboard activity:: 2>&1 | tail -8`
Expected: `test result: ok. 1 passed`。

- [ ] **Step 6: 加角落单测（追加到 `activity.rs` 的 `mod tests`，放在 `buckets_are_fixed_count_and_width` 之后）**

```rust
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
        assert_eq!(r.buckets[BUCKETS - 1].total, 1, "now falls in the last bucket");
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
        for (name, n) in [("t1", 6), ("t2", 4), ("t3", 3), ("t4", 2), ("t5", 1), ("t6", 1)] {
            for _ in 0..n {
                v.push(inp(now, 1, "ok", Some(name), None));
            }
        }
        let r = aggregate(&v, 24_000, now);
        assert_eq!(r.busiest_tools.len(), TOP_N, "top-5 only");
        assert_eq!(r.busiest_tools[0].name, "t1");
        assert_eq!(r.busiest_tools[0].count, 6);
        assert!(r.busiest_tools.iter().all(|t| t.name != "t6"), "6th tool dropped");
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
        assert_eq!(r.slowest[0].label, "search_tools", "label = meta_tool when no target");
        assert_eq!(r.slowest[0].meta_tool, "search_tools");
    }

    #[test]
    fn response_has_no_payload_content_fields() {
        let now = 100_000;
        let r = aggregate(&[inp(now, 5, "error", Some("a__x"), Some("timeout"))], 24_000, now);
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"args\""), "no args field: {json}");
        assert!(!json.contains("\"result\""), "no result field: {json}");
    }
```

Run: `cargo test -p dashboard activity:: 2>&1 | tail -8`
Expected: `test result: ok. 8 passed`。

- [ ] **Step 7: Commit**

```bash
git add crates/dashboard/src/activity.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): activity aggregate() pure fn + types + corner tests

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：后端接线 —— `CallRingSink::activity` + `api::activity` + 路由 + L3/L4 文档

**Files:**
- Modify: `crates/dashboard/src/calls.rs`、`crates/dashboard/src/api.rs`、`crates/dashboard/src/lib.rs`
- Modify: `docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`

- [ ] **Step 1: 在 `calls.rs` 写失败测试（环级聚合）**

在 `crates/dashboard/src/calls.rs` 的 `#[cfg(test)] mod tests` 内追加（复用既有 `rec`/`content` 辅助；
`CallRecord`/`MetaTool`/`CallOutcome` 已在测试作用域）：

```rust
    #[test]
    fn activity_aggregates_live_ring_window() {
        let ring = CallRingSink::new(10);
        let now = CallRecord::now_unix_ms();
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__a"), CallOutcome::Ok, now), &content());
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__a"), CallOutcome::Error, now), &content());
        let r = ring.activity(60_000);
        assert_eq!(r.buckets.len(), 24, "fixed 24 buckets");
        assert_eq!(r.total, 2);
        assert_eq!(r.errors, 1, "one Error among two");
        assert_eq!(r.busiest_tools[0].name, "gh__a");
        assert_eq!(r.busiest_tools[0].count, 2);
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"args\""), "ring path must not leak content");
    }
```

- [ ] **Step 2: 跑测确认失败（方法不存在 → 编译错误）**

Run: `cargo test -p dashboard activity_aggregates_live_ring_window 2>&1 | tail -15`
Expected: 编译失败 `no method named `activity` found for ... CallRingSink`。

- [ ] **Step 3: 在 `calls.rs` 实现 `CallRingSink::activity`**

在 `impl CallRingSink` 内（紧跟 `pub fn get(...)` 之后、`impl` 闭合 `}` 之前）加：

```rust
    /// 把 live 环聚合为 `ActivityResponse`（最近 `window_ms`，仅元数据，绝不含 args/result）。
    /// 投影窗内（及窗外，由 `aggregate` 统一按窗过滤）记录的元数据，交给纯函数 `crate::activity::aggregate`。
    pub fn activity(&self, window_ms: u64) -> crate::activity::ActivityResponse {
        let now = CallRecord::now_unix_ms();
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let inputs: Vec<crate::activity::AggInput> = ring
            .iter()
            .map(|s| {
                let r = &s.record;
                crate::activity::AggInput {
                    id: s.seq.to_string(),
                    ts_unix_ms: r.ts_unix_ms,
                    meta_tool: r.meta_tool.as_str().to_string(),
                    target_tool: r.target_tool.clone(),
                    latency_ms: r.latency_ms,
                    outcome: r.outcome.as_str().to_string(),
                    error_kind: r.error_kind.map(|s| s.to_string()),
                }
            })
            .collect();
        drop(ring);
        crate::activity::aggregate(&inputs, window_ms, now)
    }
```

- [ ] **Step 4: 跑测确认通过**

Run: `cargo test -p dashboard activity_aggregates_live_ring_window 2>&1 | tail -6`
Expected: `test result: ok. 1 passed`。

- [ ] **Step 5: 在 `api.rs` 加 `activity` + `parse_window` + 单测**

在 `crates/dashboard/src/api.rs` 的 `call_detail` 函数之后追加：

```rust
/// 解析并 clamp `window`（毫秒）：缺省 15min，范围 [1min, 24h]。解析失败 -> 缺省。
pub fn parse_window(q: &std::collections::HashMap<String, String>) -> u64 {
    q.get("window")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(900_000)
        .clamp(60_000, 86_400_000)
}

/// 活动聚合：dashboard 启用且有 live 环时聚合该环，否则返回空（24 个全 0 桶）。仅元数据。
pub fn activity(state: &AppState, window_ms: u64) -> crate::activity::ActivityResponse {
    match &state.calls {
        Some(ring) => ring.activity(window_ms),
        None => crate::activity::aggregate(&[], window_ms, observe::CallRecord::now_unix_ms()),
    }
}
```

在 `api.rs` 的 `#[cfg(test)] mod tests` 内（若无则新建于文件末尾）追加：

```rust
    #[test]
    fn parse_window_defaults_and_clamps() {
        use std::collections::HashMap;
        let mut q = HashMap::new();
        assert_eq!(super::parse_window(&q), 900_000, "default 15min");
        q.insert("window".into(), "0".into());
        assert_eq!(super::parse_window(&q), 60_000, "clamp low to 1min");
        q.insert("window".into(), "999999999999".into());
        assert_eq!(super::parse_window(&q), 86_400_000, "clamp high to 24h");
        q.insert("window".into(), "120000".into());
        assert_eq!(super::parse_window(&q), 120_000, "in-range passes through");
        q.insert("window".into(), "abc".into());
        assert_eq!(super::parse_window(&q), 900_000, "unparseable -> default");
    }
```

> 注：`api.rs` 顶部已 `use std::sync::Arc;`、`use std::path::PathBuf;` 等；`std::collections::HashMap`
> 在 `call_filter_from_query` 处已用到（如未 `use`，此处用了全限定 `std::collections::HashMap` 故无需新增 `use`）。

- [ ] **Step 6: 跑 api 单测**

Run: `cargo test -p dashboard parse_window 2>&1 | tail -6`
Expected: `test result: ok. 1 passed`。

- [ ] **Step 7: 在 `lib.rs` 暴露类型 + 加 handler + 路由**

7a. 在 `crates/dashboard/src/lib.rs` 的 `mod activity;` 那行（Task 1 加的）改为同时 `pub use`：

```rust
mod activity;
pub use activity::ActivityResponse;
```

7b. 在 `h_calls` handler 之后加新 handler（live 内存读，无需 blocking pool）：

```rust
async fn h_activity(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<ActivityResponse> {
    let window = api::parse_window(&q);
    Json(api::activity(&s, window))
}
```

7c. 在路由链里、`.route("/api/calls/{id}", ...)` 那一行之后加：

```rust
        .route("/api/activity", get(h_activity))
```

- [ ] **Step 8: 编译 + clippy 确认整 crate 通过**

Run: `cargo test -p dashboard 2>&1 | tail -6 && cargo clippy -p dashboard --all-targets -- -D warnings 2>&1 | tail -2`
Expected: 全部测试 ok（含 activity:: 8 个 + 环级 1 个 + parse_window 1 个）；clippy 0 警告。

- [ ] **Step 9: 同步 L3/L4 文档（READ 后改，忠实于代码）**

- `docs/L4-api/dashboard.md`：在端点列表加 `GET /api/activity?window=<ms>` → `ActivityResponse`（窗缺省 15min、
  clamp 1min–24h）；记录 `CallRingSink::activity`、`api::activity`/`parse_window`、`activity::aggregate` 与
  `ActivityResponse`/`ActivityBucket`/`KindCount`/`SlowCall`/`ToolCount` 各字段；强调**仅元数据、固定 24 桶、Top-5**。
- `docs/L3-details/dashboard.md`：在「调用内容捕获 / 逐条调用环」附近加「活动聚合」段：固定 24 桶（`bucket_ms=window/24`、
  窗起点对齐 24 桶、`now` 落末桶）、窗外不计、`errors=outcome!=ok`、`by_error_kind` 仅非 ok、`busiest_tools` 仅
  `target_tool`（search_tools 无 target 不计）、`slowest` 按 latency 降序 Top-5、**隐私边界**：`ActivityResponse`
  类型不含内容字段、单测断言序列化无 `args`/`result`。

- [ ] **Step 10: Commit**

```bash
git add crates/dashboard/src/calls.rs crates/dashboard/src/api.rs crates/dashboard/src/lib.rs docs/L3-details/dashboard.md docs/L4-api/dashboard.md
git commit -m "feat(dashboard): /api/activity endpoint (ring aggregate + window clamp) + docs

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：前端 —— `Sparkline.svelte` + `Activity.svelte` + 样式

**Files:**
- Create: `crates/dashboard/ui/src/lib/Sparkline.svelte`、`crates/dashboard/ui/src/lib/Activity.svelte`
- Modify: `crates/dashboard/ui/src/app.css`

> 本 task 只新增组件与样式、暂不接入页面（Task 4 接入），故构建后这两个组件未被引用是预期的——
> 直接验证「构建通过、0 警告、assets 绿」。

- [ ] **Step 1: 创建 `Sparkline.svelte`（纯内联 SVG，无依赖、无 `{@html}`）**

```svelte
<script>
  // 24 根堆叠柱：柱高 ∝ total/max；错误段红色叠在底部。inline SVG 用 currentColor/CSS 变量上色。
  let { buckets = [] } = $props();
  const W = 240, H = 46, GAP = 2, N = 24;
  const barW = (W - GAP * (N - 1)) / N;
  const max = $derived(Math.max(1, ...buckets.map((b) => b.total)));
  const totalCalls = $derived(buckets.reduce((a, b) => a + b.total, 0));
  const totalErr = $derived(buckets.reduce((a, b) => a + b.errors, 0));
  function when(t) { return new Date(t).toLocaleTimeString(); }
</script>

<svg class="spark" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img"
     aria-label={`${totalCalls} calls, ${totalErr} errors over the window`}>
  {#each buckets as b, i}
    {@const x = i * (barW + GAP)}
    {@const th = (b.total / max) * H}
    {@const eh = (b.errors / max) * H}
    <rect {x} y={H - th} width={barW} height={th} rx="1" fill="var(--accent)" opacity="0.85">
      <title>{when(b.t)} · {b.total} calls{b.errors ? `, ${b.errors} err` : ""}</title>
    </rect>
    {#if b.errors}
      <rect {x} y={H - eh} width={barW} height={eh} rx="1" fill="var(--danger)" />
    {/if}
  {/each}
</svg>
```

- [ ] **Step 2: 创建 `Activity.svelte`**

```svelte
<script>
  import { getJSON } from "./api.js";
  import { refresh } from "./refresh.svelte.js";
  import Sparkline from "./Sparkline.svelte";
  // `window` 是全局名,别名为 win 避免遮蔽。sections: 逗号分隔的 spark/breakdown/leaders。
  let { window: win, sections = "spark" } = $props();
  let data = $state(null);
  const shown = $derived(new Set(sections.split(",")));
  async function load() {
    const reqW = win; // 丢弃被新窗口取代的过期响应
    try {
      const d = await getJSON(`/api/activity?window=${win}`);
      if (reqW === win) data = d;
    } catch (_) { /* 次要数据,主错误 UI 归页面所有 */ }
  }
  $effect(() => { void win; refresh.tick; load(); });
</script>

{#if data}
  {#if shown.has("spark")}
    <div class="actpanel">
      <div class="actpanel-h">Activity · 最近 {Math.round(win / 60000)} 分钟</div>
      {#if data.total > 0}
        <Sparkline buckets={data.buckets} />
        <div class="spark-legend"><span>{data.total} calls</span>{#if data.errors}<span class="bad">{data.errors} errors</span>{/if}</div>
      {:else}
        <div class="muted">no activity yet</div>
      {/if}
    </div>
  {/if}

  {#if shown.has("breakdown") && data.by_error_kind.length}
    <div class="kindbar">
      {#each data.by_error_kind as k}<span class="tag"><span class="bad">{k.kind}</span> {k.count}</span>{/each}
    </div>
  {/if}

  {#if shown.has("leaders")}
    <div class="leadrow">
      <div class="lead">
        <div class="lead-h">最慢调用</div>
        {#each data.slowest as s}
          <a class="lead-li" href={`#/calls/${s.id}`}><span class="mono">{s.label}</span><span class="num bad">{s.latency_ms}ms</span></a>
        {/each}
        {#if !data.slowest.length}<div class="muted">—</div>{/if}
      </div>
      <div class="lead">
        <div class="lead-h">最忙工具</div>
        {#each data.busiest_tools as t}
          <a class="lead-li" href={`#/tools/${encodeURIComponent(t.name)}`}><span class="mono">{t.name}</span><span class="num">{t.count}</span></a>
        {/each}
        {#if !data.busiest_tools.length}<div class="muted">—</div>{/if}
      </div>
    </div>
  {/if}
{/if}
```

- [ ] **Step 3: 在 `app.css` 追加样式（文件末尾）**

```css

/* ---- activity insights ----------------------------------------------------- */
.actpanel { background: linear-gradient(180deg, var(--panel-2), var(--panel));
            border: 1px solid var(--border); border-radius: var(--r-lg);
            padding: var(--s4); margin-bottom: var(--s4); }
.actpanel-h { font-size: var(--fs-xs); color: var(--muted); text-transform: uppercase;
              letter-spacing: .04em; margin-bottom: var(--s3); }
.spark { width: 100%; height: 46px; display: block; }
.spark-legend { display: flex; gap: var(--s3); font-size: var(--fs-xs); color: var(--muted);
                margin-top: var(--s2); font-variant-numeric: tabular-nums; }
.kindbar { display: flex; gap: var(--s2); flex-wrap: wrap; margin: 0 0 var(--s4); }
.leadrow { display: flex; gap: var(--s3); flex-wrap: wrap; margin-bottom: var(--s4); }
.lead { flex: 1; min-width: 220px; background: linear-gradient(180deg, var(--panel-2), var(--panel));
        border: 1px solid var(--border); border-radius: var(--r-lg); padding: var(--s4); }
.lead-h { font-size: var(--fs-xs); color: var(--muted); text-transform: uppercase;
          letter-spacing: .04em; margin-bottom: var(--s2); }
.lead-li { display: flex; justify-content: space-between; gap: var(--s3); align-items: center;
           padding: 5px 6px; border-radius: var(--r-sm); color: var(--fg); transition: background .12s; }
.lead-li:hover { background: var(--hover); color: var(--fg); }
.lead-li .num { font-size: var(--fs-xs); }
```

- [ ] **Step 4: 构建 + assets 测试**

Run: `cd crates/dashboard/ui && npm run build 2>&1 | tail -6 && cd ../../.. && cargo test -p dashboard assets:: 2>&1 | grep -E '^test result:'`
Expected: 构建成功、**0 警告**；`assets::` 3 passed。（两个新组件此时未被引用属预期。）

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/ui/src/lib/Sparkline.svelte crates/dashboard/ui/src/lib/Activity.svelte crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): Sparkline + Activity components + styles

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：前端接入 —— Overview + Calls（时间范围 + error_kind 列 + 放置）

**Files:**
- Modify: `crates/dashboard/ui/src/lib/Calls.svelte`、`crates/dashboard/ui/src/lib/Overview.svelte`
- Regenerate: `crates/dashboard/ui/dist/**`

- [ ] **Step 1: `Calls.svelte` 脚本 —— 引入 Activity、加 rangeMs、query 加 since、setRange**

1a. 引入组件：把
```svelte
  import Icon from "./Icon.svelte";
```
改为
```svelte
  import Icon from "./Icon.svelte";
  import Activity from "./Activity.svelte";
```

1b. 加 `rangeMs` state：把
```svelte
  let resp = $state(null);      // CallsResponse
  let error = $state(null);
```
改为
```svelte
  let resp = $state(null);      // CallsResponse
  let error = $state(null);
  let rangeMs = $state(900000); // 时间范围(ms)；0 = all。默认 15min
```

1c. `query` 加 `since`：把
```svelte
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
    q.set("limit", String(LIMIT));
```
改为
```svelte
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
    if (rangeMs > 0) q.set("since", String(Date.now() - rangeMs));
    q.set("limit", String(LIMIT));
```

1d. 加 `setRange` 函数：把
```svelte
  function clearFilters() { meta = ""; outcome = ""; qtext = ""; argKey = ""; argVal = ""; offset = 0; }
```
改为
```svelte
  function clearFilters() { meta = ""; outcome = ""; qtext = ""; argKey = ""; argVal = ""; offset = 0; }
  function setRange(ms) { rangeMs = ms; offset = 0; }
```

- [ ] **Step 2: `Calls.svelte` 标记 —— 时间范围 chips + Activity（在指标卡之后、source chips 之前）**

把（指标卡 `{/each}` 与 `</div>` 之后、紧接的 source chips 行）
```svelte
  {/each}
</div>

<div class="chips">
  <button class="chip" class:active={source === "live"} onclick={() => setSource("live")}>live</button>
```
改为
```svelte
  {/each}
</div>

<div class="chips">
  {#each [["5m", 300000], ["15m", 900000], ["1h", 3600000], ["24h", 86400000], ["all", 0]] as [lbl, ms]}
    <button class="chip" class:active={rangeMs === ms} onclick={() => setRange(ms)}>{lbl}</button>
  {/each}
</div>
<Activity window={rangeMs > 0 ? rangeMs : 3600000} sections="spark,breakdown" />

<div class="chips">
  <button class="chip" class:active={source === "live"} onclick={() => setSource("live")}>live</button>
```

- [ ] **Step 3: `Calls.svelte` 表格 —— 加 error_kind 列**

3a. 表头：把
```svelte
      <thead><tr><th>time</th><th>meta</th><th>target</th><th>upstream</th><th>outcome</th><th class="num">ms</th></tr></thead>
```
改为
```svelte
      <thead><tr><th>time</th><th>meta</th><th>target</th><th>upstream</th><th>outcome</th><th>error</th><th class="num">ms</th></tr></thead>
```

3b. 行：把
```svelte
            <td><span class="badge {c.outcome}">{c.outcome}</span></td>
            <td class="num">{c.latency_ms}</td>
```
改为
```svelte
            <td><span class="badge {c.outcome}">{c.outcome}</span></td>
            <td><span class:bad={c.error_kind}>{c.error_kind ?? "—"}</span></td>
            <td class="num">{c.latency_ms}</td>
```

- [ ] **Step 4: `Overview.svelte` —— 引入并放置 Activity（趋势 + 双榜）**

4a. 引入：把
```svelte
  import Icon from "./Icon.svelte";
```
改为
```svelte
  import Icon from "./Icon.svelte";
  import Activity from "./Activity.svelte";
```

4b. 放置：把（数据卡片块闭合 `</div>` 之后、`{:else if !error}` 之前）
```svelte
    </div>
  </div>
{:else if !error}
```
改为
```svelte
    </div>
  </div>
  <Activity window={900000} sections="spark,leaders" />
{:else if !error}
```

- [ ] **Step 5: 构建 + assets 测试**

Run: `cd crates/dashboard/ui && npm run build 2>&1 | tail -6 && cd ../../.. && cargo test -p dashboard assets:: 2>&1 | grep -E '^test result:'`
Expected: 构建成功、**0 警告**（svelte-ignore 已覆盖既有整行点击）；`assets::` 3 passed。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): wire Activity into Overview + Calls; time-range filter + error_kind column

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5：e2e + L1/L2 文档 + 四道门禁

**Files:**
- Modify: `crates/mcpgw/tests/dashboard.rs`
- Modify: `docs/L1-overview.md`、`docs/L2-components/dashboard.md`

- [ ] **Step 1: mock 上游 e2e 加 `/api/activity` 断言**

在 `crates/mcpgw/tests/dashboard.rs` 的 `dashboard_detail_endpoints_with_mock_upstream` 测试里、
`client.cancel().await.unwrap();` 之前（即所有 M1/M2 断言之后）追加（匹配文件既有的
`.send().await.unwrap().json().await.unwrap()` 风格）：

```rust
    // Activity aggregation over the live ring: fixed 24 buckets + busiest tool.
    let act: serde_json::Value = http
        .get(format!("{base}/api/activity?window=900000"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        act["buckets"].as_array().unwrap().len(),
        24,
        "activity returns a fixed 24 buckets"
    );
    assert!(act["total"].as_u64().unwrap() >= 1, "at least the echo call_tool");
    let busy = act["busiest_tools"].as_array().unwrap();
    assert!(
        busy.iter().any(|t| t["name"] == "mock__echo"),
        "mock__echo is among busiest tools: {busy:?}"
    );
```

Run（建 mock-stdio + 强制真跑）：
`cargo build -p upstream --features testkit --bin mock-stdio && MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored 2>&1 | tail -8`
Expected: 2 passed（含本 e2e）。

- [ ] **Step 2: L2 文档（`docs/L2-components/dashboard.md`）**

在「逐条调用环 / `CallRingSink`」段后，加 `ActivityResponse` 与 `/api/activity` 的 L2 条目（READ 后照实写）：
- `CallRingSink::activity(window_ms) -> ActivityResponse`：把 live 环聚合为活动洞察（仅元数据）。
- `ActivityResponse`：`window_ms`/`bucket_ms`/`buckets[24]{t,total,errors}`/`total`/`errors`/
  `by_error_kind[{kind,count}]`/`slowest[{id,label,meta_tool,latency_ms,outcome}]`/`busiest_tools[{name,count}]`。
- `/api/*` 端点列表加 `GET /api/activity?window=<ms>`（缺省 15min、clamp 1min–24h），端点数 **11 → 12**。

- [ ] **Step 3: L1 文档（`docs/L1-overview.md`）**

- 把 dashboard 端点数 `11` 改为 `12`，并在子系统 A 那条末尾补一句：
  `+ /api/activity 活动聚合（live 环上的趋势 24 桶 + error_kind 分布 + 最慢/最忙 Top5,仅元数据）；
  前端 Overview 趋势+双榜、Calls 时间范围过滤(since)+error_kind 列+分布`。
- 测试计数行先留待 Step 4 实测回填。

- [ ] **Step 4: 四道门禁 + 计数回填**

```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿。从 `cargo test --all-features` 汇总 `N passed / M ignored`（用
`cargo test --all-features 2>&1 | awk '/^test result:/{p+=$4; i+=$8} END{print p" / "i}'`）回填 `docs/L1-overview.md`
的测试计数行。并复跑 mock e2e（2 passed）、并
`cd crates/dashboard/ui && npm run build && cd ../../.. && git status --short crates/dashboard/ui/dist`（应为空,dist 可复现）。

- [ ] **Step 5: Commit**

```bash
git add crates/mcpgw/tests/dashboard.rs docs/L1-overview.md docs/L2-components/dashboard.md
git commit -m "test+docs: /api/activity e2e + sync L1/L2

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 完成判据（DoD）

- `GET /api/activity?window=` 返回 24 桶 + Top5 双榜 + error_kind 分布,仅元数据;`window` 缺省 15min、clamp 1min–24h。
- Overview 显示趋势 + 最慢/最忙双榜;Calls 显示时间范围 chips(默认 15m,驱动表格 `since` 与 sparkline)+ error_kind 列 + 分布。
- 隐私不变量保持:`ActivityResponse`/榜单绝不含 args/result（类型层面 + 单测断言）。
- 四道门禁 + `assets::` + mock e2e 全绿;L1–L4 文档同步;dist 可复现、构建 0 警告。
- 每个 task 跑 spec+质量双审查;全部完成后整分支 audit → `--no-ff` 合并 master + push。
