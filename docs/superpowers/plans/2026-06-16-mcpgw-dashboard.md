# mcpgw 只读可视化面板（dashboard）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 mcpgw 增加一个随 `serve` 在独立端口拉起的**只读 Web 可视化面板**：展示接入的所有上游 MCP 及状态、聚合调用率/延迟，以及每个 query 选中了哪些 tool（含分数），实时 + 历史回放。

**Architecture:** 新增 `dashboard` crate（独立 tokio 任务 + 独立端口 + panic 边界 + 有界缓冲），只读 `Arc<GatewayState>` 快照 + 两个内存 sink（`MetricsSink`/`DiscoveryRingSink`，复用 `observe::CallSink` 接缝并新增 `DiscoverySink` 契约）+ 可选 JSONL 历史回放。前端是零构建 vanilla JS SPA，定时轮询只读 JSON API。

**Tech Stack:** Rust workspace、axum 0.8（已是依赖）、tokio、serde/serde_json、arc-swap（gateway 已用）；前端纯 HTML/JS/CSS（`include_str!` 内嵌，无 npm/无图表库）。

参考 spec：`docs/superpowers/specs/2026-06-16-mcpgw-dashboard-design.md`。

**全局验证门禁（每个实现 task 完成后都要过）：**
- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`（注意 `io_other_error`：用 `std::io::Error::other(...)`）
- `cargo test --all-features`
- 提交信息末尾加 `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`

---

## File Structure

| 文件 | 职责 | 本计划 |
|---|---|---|
| `crates/observe/src/discovery.rs` | `DiscoveryRecord`/`DiscoveryHit`/`DiscoverySink` 契约 | **新建**（Task 1） |
| `crates/observe/src/lib.rs` | 模块挂载 + re-export | 改（Task 1） |
| `crates/metatools/src/snapshot.rs` | `ToolSummary` 加 `score` | 改（Task 2） |
| `crates/metatools/src/tools.rs` | `search_tools` 带出 score | 改（Task 2） |
| `crates/config/src/lib.rs` | `[dashboard]` 段 + validate | 改（Task 3） |
| `crates/gateway/src/lib.rs` | 存最近 `RebuildSummary` + `last_summary()` | 改（Task 4） |
| `crates/dashboard/Cargo.toml` + `src/metrics.rs` | `MetricsSink`(impl CallSink) + `MetricsSnapshot` | **新建**（Task 5） |
| `crates/dashboard/src/trace.rs` | `DiscoveryRingSink`(impl DiscoverySink) + 可选 writer | **新建**（Task 6） |
| `crates/dashboard/src/history.rs` | 有界 JSONL 回放（审计 + discovery） | **新建**（Task 7） |
| `crates/downstream/src/lib.rs` | 注入 discovery sinks + 在 search_tools 捕获 | 改（Task 8） |
| `crates/dashboard/src/lib.rs` + `assets/*` | JSON API + router + 静态前端 | **新建**（Task 9-10） |
| `crates/mcpgw/src/main.rs` | 装配：wire sinks、起 dashboard 任务、配置 | 改（Task 11） |
| `docs/*` | L1–L4 同步 + 测试计数 | 改（Task 12） |
| —（验证/合并） | 门禁 + 终审 + 合并 + 推送 | Task 13 |

> 数据来源补充：上游"connected/skipped+原因"取自 gateway 新增的 `last_summary()`（Task 4）；上游 transport 取自 Config（Task 11 装配时传入 dashboard）；每上游工具数由快照 catalog 按 `server` 字段聚合。

---

## Task 1: observe — `DiscoverySink` 契约（`DiscoveryRecord` / `DiscoveryHit` / trait）

**Files:**
- Create: `crates/observe/src/discovery.rs`
- Modify: `crates/observe/src/lib.rs`（挂模块 + re-export）
- Test: `crates/observe/src/discovery.rs` 的 `#[cfg(test)] mod tests`

背景：富追踪与隐私洁净的 `CallRecord` 物理隔离——新增独立的 `DiscoverySink` 契约（与 `CallSink` 并列），承载 query 原文 + 工具名 + 分数。本 task 只立契约与序列化形状，不接捕获、不接 UI。

- [ ] **Step 1: 写失败测试**

新建 `crates/observe/src/discovery.rs`，先写测试（顶部用 `use super::*;`）：

```rust
//! Optional, opt-in capture of search/discovery traces (query -> selected tools + scores).
//! Kept SEPARATE from the metadata-only `CallRecord` so query text never leaks into the
//! privacy-clean call sinks (tracing/audit).

use serde::Serialize;

/// One returned tool in a discovery trace: its namespaced name and relevance score.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveryHit {
    pub name: String,
    pub score: f32,
}

/// One `search_tools` call: the raw query and the tools it surfaced (with scores).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveryRecord {
    pub ts_unix_ms: u64,
    pub query: String,
    pub top_k: usize,
    pub results: Vec<DiscoveryHit>,
    pub latency_ms: u64,
}

/// Fan-out target for discovery traces. Implemented by the dashboard's in-memory ring buffer.
pub trait DiscoverySink: Send + Sync {
    fn record(&self, rec: &DiscoveryRecord);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn discovery_record_serializes_with_expected_keys() {
        let rec = DiscoveryRecord {
            ts_unix_ms: 1,
            query: "weather".into(),
            top_k: 2,
            results: vec![DiscoveryHit { name: "w__get".into(), score: 1.5 }],
            latency_ms: 3,
        };
        let v: serde_json::Value = serde_json::to_value(&rec).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<_> = obj.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["latency_ms", "query", "results", "top_k", "ts_unix_ms"]);
        assert_eq!(obj["results"][0]["name"], "w__get");
        assert_eq!(obj["results"][0]["score"], 1.5);
    }

    #[test]
    fn discovery_sink_receives_records() {
        struct Collect(Mutex<Vec<DiscoveryRecord>>);
        impl DiscoverySink for Collect {
            fn record(&self, rec: &DiscoveryRecord) {
                self.0.lock().unwrap().push(rec.clone());
            }
        }
        let sink = Collect(Mutex::new(Vec::new()));
        let rec = DiscoveryRecord {
            ts_unix_ms: 0,
            query: "q".into(),
            top_k: 1,
            results: vec![],
            latency_ms: 0,
        };
        sink.record(&rec);
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p observe discovery 2>&1 | tail -20`
Expected: FAIL —— `discovery` 模块未挂载（编译错误：`file not found for module` / 未 re-export）。

- [ ] **Step 3: 最小实现（挂模块 + re-export）**

在 `crates/observe/src/lib.rs` 顶部（其它 `mod`/`pub use` 附近）加：

```rust
mod discovery;
pub use discovery::{DiscoveryHit, DiscoveryRecord, DiscoverySink};
```

（`discovery.rs` 的类型/trait 已在 Step 1 随测试一并写好。）

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p observe`
Expected: PASS —— 新增 2 个 discovery 测试 + 现有 observe 测试全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p observe --all-targets --all-features -- -D warnings && cargo test -p observe
git add crates/observe/src/discovery.rs crates/observe/src/lib.rs
git commit -m "feat(observe): add DiscoverySink contract for query->tools traces (dashboard)

A separate, opt-in capture channel for search/discovery traces (query + selected
tool names + scores), kept apart from the metadata-only CallRecord so query text
never reaches the privacy-clean tracing/audit sinks.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: metatools — `ToolSummary` 带相关性分数

**Files:**
- Modify: `crates/metatools/src/snapshot.rs`（`ToolSummary` 加 `score`）
- Modify: `crates/metatools/src/tools.rs`（`search_tools` 带出 `score` + 新测试）
- Test: `crates/metatools/src/tools.rs` 的 `mod tests`

背景：`search_tools` 当前把 `retrieval::ScoredTool.score` 丢弃。给 `ToolSummary` 加 `score: f32`（对 MCP 客户端向后兼容的加字段），供面板 discovery 追踪与客户端共用。

- [ ] **Step 1: 写失败测试**

在 `crates/metatools/src/tools.rs` 的 `mod tests` 内（`search_tools_returns_namespaced_summaries` 之后）加：

```rust
    #[tokio::test]
    async fn search_tools_carries_descending_scores() {
        let snap = snapshot().await;
        let hits = search_tools(&snap, "weather forecast", 5).await;
        assert!(!hits.is_empty());
        assert!(hits[0].score > 0.0, "top hit has a positive score");
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "scores must be descending");
        }
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p metatools search_tools_carries_descending_scores 2>&1 | tail -20`
Expected: FAIL —— 编译错误：`ToolSummary` 无 `score` 字段。

- [ ] **Step 3: 最小实现**

在 `crates/metatools/src/snapshot.rs` 的 `ToolSummary` 加字段：

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    pub score: f32,
}
```

在 `crates/metatools/src/tools.rs` 的 `search_tools` map 中带出分数：

```rust
        .map(|hit| ToolSummary {
            name: hit.qualified_name,
            description: hit.description,
            score: hit.score,
        })
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p metatools`
Expected: PASS —— 新测试 + 现有 metatools 测试全绿（现有测试只读 `.name`/`.description`，不受加字段影响）。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p metatools --all-targets --all-features -- -D warnings && cargo test -p metatools
git add crates/metatools/src/snapshot.rs crates/metatools/src/tools.rs
git commit -m "feat(metatools): carry relevance score on ToolSummary (dashboard)

search_tools dropped the retrieval score; thread it onto ToolSummary (additive,
backward-compatible for MCP clients) so the dashboard's discovery trace and
clients can both see relevance.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: config — `[dashboard]` 段

**Files:**
- Modify: `crates/config/src/lib.rs`（新增 `DashboardConfig` + `Config.dashboard` + `validate()` + 测试）
- Test: `crates/config/src/lib.rs` 的 `mod tests`

- [ ] **Step 1: 写失败测试**

在 `crates/config/src/lib.rs` 的 `mod tests` 内加：

```rust
    #[test]
    fn dashboard_defaults_and_partial_fill() {
        let cfg = Config::from_toml_str("[dashboard]\nenabled = true\n").unwrap();
        assert!(cfg.dashboard.enabled);
        assert_eq!(cfg.dashboard.bind, "127.0.0.1:8971");
        assert!(!cfg.dashboard.trace_queries);
        assert_eq!(cfg.dashboard.trace_path, None);
        assert_eq!(cfg.dashboard.trace_buffer, 500);
    }

    #[test]
    fn omitting_dashboard_section_is_disabled() {
        assert!(!Config::from_toml_str("").unwrap().dashboard.enabled);
    }

    #[test]
    fn dashboard_rejects_unknown_field_and_zero_buffer() {
        assert!(matches!(
            Config::from_toml_str("[dashboard]\nbogus = 1\n").unwrap_err(),
            ConfigError::Parse(_)
        ));
        assert!(matches!(
            Config::from_toml_str("[dashboard]\nenabled = true\ntrace_buffer = 0\n").unwrap_err(),
            ConfigError::Invalid(_)
        ));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p config dashboard 2>&1 | tail -20`
Expected: FAIL —— 编译错误：`Config` 无 `dashboard` 字段 / `DashboardConfig` 未定义。

- [ ] **Step 3: 最小实现**

在 `crates/config/src/lib.rs` 给 `Config` 加字段（`audit` 之后）：

```rust
    #[serde(default)]
    pub dashboard: DashboardConfig,
```

新增结构体（放在 `AuditConfig` 定义之后）：

```rust
/// `[dashboard]` section: optional read-only web dashboard (subsystem A).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    /// Start the dashboard HTTP server. Defaults to false (must opt in).
    pub enabled: bool,
    /// Bind address. Localhost only; no auth. Defaults to 127.0.0.1:8971.
    pub bind: String,
    /// Capture query text + selected tool names/scores for the trace view (opt-in).
    pub trace_queries: bool,
    /// Optional discovery JSONL path for history replay. None -> in-memory ring buffer only.
    pub trace_path: Option<String>,
    /// In-memory discovery ring buffer size. Must be > 0.
    pub trace_buffer: usize,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1:8971".into(),
            trace_queries: false,
            trace_path: None,
            trace_buffer: 500,
        }
    }
}
```

在 `validate()` 末尾（`Ok(())` 之前）加：

```rust
        if self.dashboard.enabled {
            if self.dashboard.bind.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "[dashboard].bind must not be empty when enabled".into(),
                ));
            }
            if self.dashboard.trace_buffer == 0 {
                return Err(ConfigError::Invalid(
                    "[dashboard].trace_buffer must be > 0".into(),
                ));
            }
        }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p config`
Expected: PASS —— 3 个新测试 + 现有 config 测试全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p config --all-targets --all-features -- -D warnings && cargo test -p config
git add crates/config/src/lib.rs
git commit -m "feat(config): add [dashboard] section (dashboard subsystem A)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: gateway — 保留最近一次 `RebuildSummary`

**Files:**
- Modify: `crates/gateway/src/lib.rs`（`GatewayState` 加 `last_summary` 字段 + `last_summary()` 访问器 + `rebuild_snapshot` 末尾存储 + 测试）
- Test: `crates/gateway/src/lib.rs` 的 `#[cfg(test)] mod tests`

背景：面板的上游视图要展示 connected/skipped+原因，这正是 `RebuildSummary` 的内容，但目前它只被 `rebuild_snapshot` 返回、不持久。加一个无锁可读的"最近一次摘要"。

- [ ] **Step 1: 写失败测试**

在 `crates/gateway/src/lib.rs` 的 `mod tests` 内加：

```rust
    #[tokio::test]
    async fn last_summary_is_none_until_first_rebuild() {
        let state = GatewayState::new("bm25").unwrap();
        assert!(state.last_summary().is_none());
        let _ = state.rebuild_snapshot().await.unwrap();
        let s = state.last_summary().expect("summary recorded after rebuild");
        assert!(s.ingested.is_empty() && s.skipped.is_empty()); // no upstreams registered
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p gateway last_summary_is_none_until_first_rebuild 2>&1 | tail -20`
Expected: FAIL —— 编译错误：无 `last_summary` 方法。

- [ ] **Step 3: 最小实现**

在 `crates/gateway/src/lib.rs` 顶部 `use` 区加（与现有 `use arc_swap::ArcSwap;` 并列）：

```rust
use arc_swap::ArcSwapOption;
```

给 `GatewayState` 加字段（`rebuild_lock` 之后）：

```rust
    /// Most recent rebuild summary (ingested/skipped upstreams), for the dashboard. Read lock-free.
    last_summary: Arc<ArcSwapOption<RebuildSummary>>,
```

在 `build()` 的构造体里加初值：

```rust
            last_summary: Arc::new(ArcSwapOption::empty()),
```

在 `rebuild_snapshot` 末尾，`Ok(summary)` 之前，存储一份：

```rust
        self.last_summary.store(Some(Arc::new(summary.clone())));
        Ok(summary)
```

在 `impl GatewayState` 内加访问器（紧邻 `snapshot()`）：

```rust
    /// The most recent rebuild summary, or `None` before the first rebuild. Read lock-free.
    pub fn last_summary(&self) -> Option<Arc<RebuildSummary>> {
        self.last_summary.load_full()
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p gateway`
Expected: PASS —— 新测试 + 现有 gateway 测试全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p gateway --all-targets --all-features -- -D warnings && cargo test -p gateway
git add crates/gateway/src/lib.rs
git commit -m "feat(gateway): retain the latest RebuildSummary for the dashboard

Expose last_summary() (lock-free ArcSwapOption) so the dashboard can show which
upstreams were ingested vs skipped (with reasons) on the most recent rebuild.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: dashboard crate骨架 + `MetricsSink`（实现 `observe::CallSink`）

**Files:**
- Create: `crates/dashboard/Cargo.toml`
- Create: `crates/dashboard/src/lib.rs`（先只 `mod metrics; pub use ...;`，router 在 Task 9 补）
- Create: `crates/dashboard/src/metrics.rs`
- Modify: 根 `Cargo.toml`（workspace members 追加 `crates/dashboard`）
- Test: `crates/dashboard/src/metrics.rs` 的 `mod tests`

背景：进程内聚合每条 `CallRecord` 为可序列化的 `MetricsSnapshot`（总调用数、每元工具 calls/errors/p50/p95/max、每上游 calls/errors）。桶/键数有限→内存有界；锁不跨 `.await`。

- [ ] **Step 1: 建 crate 骨架（使其能编译）**

新建 `crates/dashboard/Cargo.toml`：

```toml
[package]
name = "dashboard"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
gateway = { path = "../gateway" }
observe = { path = "../observe" }
catalog = { path = "../catalog" }
config = { path = "../config" }
axum = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["full"] }
```

根 `Cargo.toml` 的 `members` 末尾追加 `"crates/dashboard"`。

新建 `crates/dashboard/src/lib.rs`：

```rust
//! Read-only web dashboard for mcpgw (subsystem A): metrics aggregation, discovery traces,
//! and a static SPA served over a separate localhost port.

mod metrics;
pub use metrics::{MetaToolMetrics, MetricsSink, MetricsSnapshot, UpstreamMetrics};
```

- [ ] **Step 2: 写失败测试**

新建 `crates/dashboard/src/metrics.rs`，先写测试：

```rust
use observe::{CallOutcome, CallRecord, CallSink, MetaTool};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;

// ... (implementation added in Step 3) ...

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(meta: MetaTool, outcome: CallOutcome, latency: u64, upstream: Option<&str>) -> CallRecord {
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
        sink.record(&rec(MetaTool::CallTool, CallOutcome::Ok, 10, Some("github")));
        sink.record(&rec(MetaTool::CallTool, CallOutcome::Error, 30, Some("github")));
        sink.record(&rec(MetaTool::SearchTools, CallOutcome::Ok, 5, None));
        let snap = sink.snapshot();
        assert_eq!(snap.total_calls, 3);
        let ct = snap.per_meta_tool.iter().find(|m| m.meta_tool == "call_tool").unwrap();
        assert_eq!(ct.calls, 2);
        assert_eq!(ct.errors, 1);
        assert_eq!(ct.max_ms, 30);
        let gh = snap.per_upstream.iter().find(|u| u.upstream == "github").unwrap();
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
        let s = snap.per_meta_tool.iter().find(|m| m.meta_tool == "search_tools").unwrap();
        assert!(s.p50_ms <= s.p95_ms, "p50 <= p95");
        assert!(s.p95_ms <= s.max_ms, "p95 <= max");
        assert_eq!(s.max_ms, 400);
    }

    #[test]
    fn empty_snapshot_is_zeroed() {
        let snap = MetricsSink::new().snapshot();
        assert_eq!(snap.total_calls, 0);
        assert!(snap.per_meta_tool.is_empty());
        assert!(snap.per_upstream.is_empty());
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p dashboard metrics 2>&1 | tail -20`
Expected: FAIL —— `MetricsSink` 等未定义（编译错误）。

- [ ] **Step 4: 最小实现**

在 `crates/dashboard/src/metrics.rs` 的测试模块**之前**插入实现：

```rust
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
        let idx = BUCKETS_MS.iter().position(|&b| latency_ms <= b).unwrap_or(BUCKETS_MS.len() - 1);
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

/// In-memory aggregator of `CallRecord`s. Implements `observe::CallSink`; bounded (fixed bucket
/// and key sets). The lock is a plain `Mutex` held only for the short aggregation/snapshot work.
pub struct MetricsSink {
    state: Mutex<MetricsState>,
}

impl MetricsSink {
    pub fn new() -> Self {
        Self { state: Mutex::new(MetricsState::default()) }
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
        let is_error = matches!(rec.outcome, CallOutcome::Error);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.total += 1;
        st.per_meta.entry(rec.meta_tool.as_str()).or_default().observe(rec.latency_ms, is_error);
        if let Some(up) = &rec.upstream {
            let agg = st.per_upstream.entry(up.clone()).or_default();
            agg.calls += 1;
            if is_error {
                agg.errors += 1;
            }
        }
    }
}
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p dashboard`
Expected: PASS —— 3 个 metrics 测试全绿。

- [ ] **Step 6: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings && cargo test -p dashboard
git add Cargo.toml crates/dashboard/Cargo.toml crates/dashboard/src/lib.rs crates/dashboard/src/metrics.rs
git commit -m "feat(dashboard): MetricsSink aggregating CallRecords (counts/errors/p50/p95)

New dashboard crate skeleton + an in-memory observe::CallSink that aggregates
per-meta-tool and per-upstream call metrics into a serializable MetricsSnapshot.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6: dashboard — `DiscoveryRingSink`（内存 ring buffer + 可选 discovery JSONL）

**Files:**
- Create: `crates/dashboard/src/trace.rs`
- Modify: `crates/dashboard/src/lib.rs`（挂 `mod trace;` + re-export）
- Test: `crates/dashboard/src/trace.rs` 的 `mod tests`

背景：实现 `observe::DiscoverySink`——内存 ring buffer（最近 N 条，满则覆盖最旧）给实时；可选地把每条记录经**有界 channel + 后台线程**追加到 discovery JSONL（满则计数丢弃、不阻塞调用），并提供可在关停时 join 的 writer 句柄（与 audit 同模式）。

- [ ] **Step 1: 写失败测试**

新建 `crates/dashboard/src/trace.rs`，先写测试：

```rust
use observe::{DiscoveryHit, DiscoveryRecord, DiscoverySink};
use std::collections::VecDeque;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;

// ... (implementation added in Step 3) ...

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(q: &str) -> DiscoveryRecord {
        DiscoveryRecord {
            ts_unix_ms: 0,
            query: q.into(),
            top_k: 1,
            results: vec![DiscoveryHit { name: "s__t".into(), score: 1.0 }],
            latency_ms: 0,
        }
    }

    #[test]
    fn ring_caps_and_returns_newest_first() {
        let (sink, _w) = DiscoveryRingSink::spawn(2, None).unwrap();
        sink.record(&rec("a"));
        sink.record(&rec("b"));
        sink.record(&rec("c")); // evicts "a"
        let recent = sink.recent(10);
        let queries: Vec<_> = recent.iter().map(|r| r.query.as_str()).collect();
        assert_eq!(queries, ["c", "b"], "newest first, capacity 2");
    }

    #[test]
    fn recent_respects_limit() {
        let (sink, _w) = DiscoveryRingSink::spawn(10, None).unwrap();
        for q in ["a", "b", "c"] {
            sink.record(&rec(q));
        }
        assert_eq!(sink.recent(2).len(), 2);
    }

    #[test]
    fn file_writer_persists_lines_then_drains_on_join() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mcpgw-disc-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (sink, writer) = DiscoveryRingSink::spawn(10, Some(&path)).unwrap();
        sink.record(&rec("x"));
        sink.record(&rec("y"));
        drop(sink); // release the sender so the writer thread can finish
        writer.expect("writer present when path given").join();
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["query"], "x");
        let _ = std::fs::remove_file(&path);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p dashboard trace 2>&1 | tail -20`
Expected: FAIL —— `DiscoveryRingSink` 未定义 / 模块未挂。

- [ ] **Step 3: 最小实现**

在 `crates/dashboard/src/trace.rs` 测试模块**之前**插入：

```rust
const WRITER_CHANNEL_CAP: usize = 1024;

/// Join handle for the optional discovery-JSONL writer thread. Dropping all `DiscoveryRingSink`
/// clones closes the channel; `join()` then drains and flushes remaining lines.
pub struct DiscoveryWriter {
    handle: std::thread::JoinHandle<()>,
}

impl DiscoveryWriter {
    pub fn join(self) {
        let _ = self.handle.join();
    }
}

/// In-memory ring buffer of recent discovery traces (newest-first on read), with an optional
/// background writer appending each record as a JSON line to a discovery JSONL file.
pub struct DiscoveryRingSink {
    cap: usize,
    ring: Mutex<VecDeque<DiscoveryRecord>>,
    tx: Option<SyncSender<String>>,
    dropped: AtomicU64,
}

impl DiscoveryRingSink {
    /// Build a ring sink (capacity `cap`). When `path` is `Some`, also append records to that
    /// discovery JSONL via a background writer thread (returned for graceful drain on shutdown).
    pub fn spawn(cap: usize, path: Option<&Path>) -> std::io::Result<(Self, Option<DiscoveryWriter>)> {
        let cap = cap.max(1);
        let (tx, writer) = match path {
            None => (None, None),
            Some(p) => {
                let file = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
                let (tx, rx) = sync_channel::<String>(WRITER_CHANNEL_CAP);
                let handle = std::thread::Builder::new()
                    .name("discovery-writer".into())
                    .spawn(move || run_writer(rx, file))?;
                (Some(tx), Some(DiscoveryWriter { handle }))
            }
        };
        Ok((Self { cap, ring: Mutex::new(VecDeque::with_capacity(cap)), tx, dropped: AtomicU64::new(0) }, writer))
    }

    /// Most recent records, newest first, capped at `limit`.
    pub fn recent(&self, limit: usize) -> Vec<DiscoveryRecord> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().rev().take(limit).cloned().collect()
    }

    /// Count of records dropped because the writer channel was full (test/diagnostics).
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl DiscoverySink for DiscoveryRingSink {
    fn record(&self, rec: &DiscoveryRecord) {
        {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            if ring.len() == self.cap {
                ring.pop_front();
            }
            ring.push_back(rec.clone());
        }
        if let Some(tx) = &self.tx {
            if let Ok(line) = serde_json::to_string(rec) {
                if let Err(TrySendError::Full(_)) = tx.try_send(line) {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Append each received line + flush; on clean disconnect, flush and fsync once before exit.
fn run_writer(rx: Receiver<String>, file: std::fs::File) {
    let mut w = BufWriter::new(file);
    while let Ok(line) = rx.recv() {
        if writeln!(w, "{line}").is_err() {
            continue;
        }
        while let Ok(next) = rx.try_recv() {
            let _ = writeln!(w, "{next}");
        }
        let _ = w.flush();
    }
    let _ = w.flush();
    if let Ok(file) = w.into_inner() {
        let _ = file.sync_all();
    }
}
```

在 `crates/dashboard/src/lib.rs` 加：

```rust
mod trace;
pub use trace::{DiscoveryRingSink, DiscoveryWriter};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p dashboard`
Expected: PASS —— 3 个 trace 测试 + Task 5 的 metrics 测试全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings && cargo test -p dashboard
git add crates/dashboard/src/trace.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): DiscoveryRingSink (bounded ring + optional discovery JSONL)

In-memory ring buffer of recent query->tools traces (newest-first), with an
optional background writer appending each record as a JSON line for history
replay; bounded channel drops on full and never blocks the call path.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 7: dashboard — `history`（有界 JSONL 回放：审计指标 + discovery 追踪）

**Files:**
- Modify: `crates/observe/src/discovery.rs`（给 `DiscoveryRecord`/`DiscoveryHit` 加 `Deserialize`，供回放反序列化）
- Create: `crates/dashboard/src/history.rs`
- Modify: `crates/dashboard/src/lib.rs`（挂 `mod history;` + re-export）
- Test: `crates/dashboard/src/history.rs` 的 `mod tests`

背景：历史视图按需读取已有 JSONL——audit JSONL 重建分时段调用率/错误数，discovery JSONL 回放 query→tools。读取**限量**（尾部 N 行，内存有界），文件缺失/坏行优雅降级。

- [ ] **Step 1: 让 `DiscoveryRecord` 可反序列化**

把 `crates/observe/src/discovery.rs` 顶部 `use serde::Serialize;` 改为：

```rust
use serde::{Deserialize, Serialize};
```

并把 `DiscoveryHit` 与 `DiscoveryRecord` 两处 `#[derive(Debug, Clone, PartialEq, Serialize)]` 各加上 `Deserialize`：

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
```

（observe 现有 discovery 测试不受影响。）

- [ ] **Step 2: 写失败测试**

新建 `crates/dashboard/src/history.rs`，先写测试：

```rust
use observe::DiscoveryRecord;
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::Path;

// ... (implementation added in Step 4) ...

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
        assert_eq!(buckets[0], MetricBucket { bucket_start_ms: 0, calls: 2, errors: 1 });
        assert_eq!(buckets[1], MetricBucket { bucket_start_ms: 1000, calls: 1, errors: 0 });
        let _ = std::fs::remove_file(&p);
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p dashboard history 2>&1 | tail -20`
Expected: FAIL —— `replay_discovery`/`MetricBucket` 未定义 / 模块未挂。

- [ ] **Step 4: 最小实现**

在 `crates/dashboard/src/history.rs` 测试模块**之前**插入：

```rust
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
    let mut recs: Vec<DiscoveryRecord> =
        lines.iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
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
pub fn replay_audit_metrics(path: &Path, limit: usize, bucket_ms: u64) -> (Vec<MetricBucket>, bool) {
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
            if a.outcome == "error" {
                e.1 += 1;
            }
        }
    }
    let out = buckets
        .into_iter()
        .map(|(start, (calls, errors))| MetricBucket { bucket_start_ms: start, calls, errors })
        .collect();
    (out, true)
}
```

在 `crates/dashboard/src/lib.rs` 加：

```rust
mod history;
pub use history::{replay_audit_metrics, replay_discovery, MetricBucket};
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p dashboard && cargo test -p observe`
Expected: PASS —— history 3 个测试 + observe（含 discovery 仍绿）全过。

- [ ] **Step 6: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p dashboard -p observe --all-targets --all-features -- -D warnings && cargo test -p dashboard -p observe
git add crates/observe/src/discovery.rs crates/dashboard/src/history.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): bounded JSONL history replay (audit metrics + discovery)

Tail up to N lines (memory-bounded) of the audit JSONL into time buckets and the
discovery JSONL into newest-first traces; missing/bad lines degrade gracefully.
Adds Deserialize to DiscoveryRecord for replay.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 8: downstream — 注入 discovery sinks + 在 `search_tools` 捕获

**Files:**
- Modify: `crates/downstream/src/lib.rs`（`GatewayServer` 加 `discovery` 字段 + `new` 加参 + 纯 helper + search 分支捕获 + 单测）
- Modify: `crates/downstream/src/http.rs`（`build_router` 加 `discovery` 参 + 传入闭包）
- Modify: `crates/downstream/tests/common/mod.rs`、`crates/downstream/tests/http_server.rs`（调用处补空 discovery 切片）
- Modify: `crates/mcpgw/src/main.rs`（两处调用补**空** discovery 切片，保持编译/测试绿；Task 11 再换成真的）
- Test: `crates/downstream/src/lib.rs` 的 `mod tests`

背景：`GatewayServer::new`/`build_router` 新增一个 `Arc<[Arc<dyn observe::DiscoverySink>]>` 参数（空切片=不捕获），`search_tools` 分支在 discovery 非空时构造并扇出 `DiscoveryRecord`。本 task 把所有调用处更新为空切片以保持每步绿；真正注入在 Task 11。

- [ ] **Step 1: 写失败测试（纯 helper）**

在 `crates/downstream/src/lib.rs` 的 `#[cfg(test)] mod tests` 内加：

```rust
    #[test]
    fn discovery_record_maps_query_and_scored_hits() {
        let hits = vec![
            metatools::ToolSummary { name: "a__x".into(), description: "d".into(), score: 2.0 },
            metatools::ToolSummary { name: "b__y".into(), description: "d".into(), score: 1.0 },
        ];
        let rec = discovery_record_for_search("find", 5, &hits, 7);
        assert_eq!(rec.query, "find");
        assert_eq!(rec.top_k, 5);
        assert_eq!(rec.latency_ms, 7);
        assert_eq!(rec.results.len(), 2);
        assert_eq!(rec.results[0].name, "a__x");
        assert_eq!(rec.results[0].score, 2.0);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p downstream discovery_record_maps 2>&1 | tail -20`
Expected: FAIL —— `discovery_record_for_search` 未定义 / `ToolSummary` 无 `score`（若 Task 2 未先做则也会提示；本计划 Task 2 在前）。

- [ ] **Step 3: 最小实现（helper + 字段 + 捕获 + 签名）**

在 `crates/downstream/src/lib.rs` 顶部 `use` 处确保有 `use std::sync::Arc;`（已存在）。给 `GatewayServer` 加字段：

```rust
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<GatewayState>,
    default_top_k: usize,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
    discovery: Arc<[Arc<dyn observe::DiscoverySink>]>,
}
```

`new` 加参：

```rust
    pub fn new(
        state: Arc<GatewayState>,
        default_top_k: usize,
        sinks: Arc<[Arc<dyn observe::CallSink>]>,
        discovery: Arc<[Arc<dyn observe::DiscoverySink>]>,
    ) -> Self {
        Self {
            state,
            default_top_k,
            sinks,
            discovery,
        }
    }
```

在 `classify` 函数附近（模块级，非 impl）加纯 helper：

```rust
/// Build a discovery trace from a completed `search_tools` call (pure; used when discovery sinks
/// are attached).
fn discovery_record_for_search(
    query: &str,
    top_k: usize,
    hits: &[metatools::ToolSummary],
    latency_ms: u64,
) -> observe::DiscoveryRecord {
    observe::DiscoveryRecord {
        ts_unix_ms: observe::CallRecord::now_unix_ms(),
        query: query.to_string(),
        top_k,
        results: hits
            .iter()
            .map(|h| observe::DiscoveryHit { name: h.name.clone(), score: h.score })
            .collect(),
        latency_ms,
    }
}
```

在 `call_tool` 的 `"search_tools" =>` 分支里，把 `let hits = metatools::search_tools(&snap, query, top_k).await;` 之后、`match serde_json::to_string(&hits)` 之前插入捕获：

```rust
                if !self.discovery.is_empty() {
                    let drec = discovery_record_for_search(
                        query,
                        top_k,
                        &hits,
                        started.elapsed().as_millis() as u64,
                    );
                    for sink in self.discovery.iter() {
                        sink.record(&drec);
                    }
                }
```

**同时修复 `upstream` 归因的安全隐患（audit pass-2 后续 / dashboard 评审发现）**：当前 `upstream` 由
`target_tool.split_once("__")` 派生，而 `target_tool` 在 `ToolNotFound` 路径上是**客户端原样提供的名字**，故
一个恶意客户端用 `aaaa0001__x`、`aaaa0002__x`… 反复调 `call_tool` 会让 `upstream` 取到无界的不同前缀（被
内存聚合器 `MetricsSink` 分桶即内存 DoS）。把 record 构造处（`let upstream = target_tool...` 那几行）改为
**只对已解析的工具归因 upstream**——经快照 catalog 查名取真实 `server`，查不到则 `None`（顺带避免「裸拆 `__`」）：

```rust
        let upstream = target_tool.as_deref().and_then(|t| {
            self.state.snapshot().catalog.get(t).map(|def| def.server.clone())
        });
```

这样 `upstream` 恒为配置内的真实 server（受 ingest 上限约束），消除客户端可控的无界值。若现有 downstream 测试
断言了 `ToolNotFound` 情况下的 `upstream` 值，相应更新为 `None`。

在 `crates/downstream/src/http.rs` 的 `build_router` 加参并传入闭包：

```rust
pub fn build_router(
    state: Arc<GatewayState>,
    default_top_k: usize,
    path: &str,
    api_keys: Vec<String>,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
    discovery: Arc<[Arc<dyn observe::DiscoverySink>]>,
) -> axum::Router {
    let service = StreamableHttpService::new(
        move || {
            Ok(GatewayServer::new(
                state.clone(),
                default_top_k,
                sinks.clone(),
                discovery.clone(),
            ))
        },
        ...
    );
    ...
}
```

- [ ] **Step 4: 更新所有调用处补空 discovery 切片（保持绿）**

`crates/downstream/tests/common/mod.rs:36`：

```rust
    let discovery: std::sync::Arc<[std::sync::Arc<dyn observe::DiscoverySink>]> =
        std::sync::Arc::from(Vec::new());
    let server = GatewayServer::new(state, default_top_k, sinks, discovery);
```

`crates/downstream/tests/http_server.rs` 第 31、154 行的 `build_router(...)` 各在末尾加一个空切片实参：

```rust
    let discovery: std::sync::Arc<[std::sync::Arc<dyn observe::DiscoverySink>]> =
        std::sync::Arc::from(Vec::new());
    let router = downstream::http::build_router(state, 8, "/mcp", api_keys, sinks, discovery);
```
（154 行那处把 `api_keys` 换成 `vec![]`，其余同。）

`crates/mcpgw/src/main.rs` 的 `build_router(...)`（~311）与 `GatewayServer::new(...)`（~347）各加末尾实参，**暂用空切片**（Task 11 替换为真实 discovery sinks）：

```rust
    let no_discovery: Arc<[Arc<dyn observe::DiscoverySink>]> = Arc::from(Vec::new());
    // build_router(state.clone(), cfg.retrieval.top_k, &h.path, api_keys, sinks.clone(), no_discovery.clone())
    // GatewayServer::new(state_for_stdio, top_k, sinks.clone(), no_discovery.clone())
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p downstream --all-features && cargo build --all-targets`
Expected: PASS —— 新 helper 单测 + 现有 downstream 测试全绿；整个工作区编译通过（含 mcpgw）。

- [ ] **Step 6: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
git add crates/downstream/src/lib.rs crates/downstream/src/http.rs crates/downstream/tests/common/mod.rs crates/downstream/tests/http_server.rs crates/mcpgw/src/main.rs
git commit -m "feat(downstream): capture query->tools discovery traces in search_tools

GatewayServer/build_router take an optional DiscoverySink slice; the search_tools
arm builds a DiscoveryRecord (query + scored hits + latency) and fans out when
attached. All call sites pass an empty slice for now (real sinks wired in assembly).

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 9: dashboard — JSON API（pure 逻辑 + axum 薄封装 + router）

**Files:**
- Create: `crates/dashboard/src/api.rs`（`AppState`、view 类型、pure 函数 + 测试）
- Modify: `crates/dashboard/src/lib.rs`（挂 `mod api;`、`pub use`、`build_dashboard_router`）
- Test: `crates/dashboard/src/api.rs` 的 `mod tests`

设计要点：每个端点的逻辑写成**纯函数**（`fn xxx(state: &AppState, ...) -> 可序列化`），axum handler 只做薄封装——这样单测无需起 HTTP/引 tower。

- [ ] **Step 1: 写失败测试**

新建 `crates/dashboard/src/api.rs`，先写测试（实现见 Step 3）：

```rust
use crate::metrics::{MetricsSink, MetricsSnapshot};
use crate::trace::DiscoveryRingSink;
use crate::history::{replay_audit_metrics, replay_discovery, MetricBucket};
use gateway::GatewayState;
use observe::CallSink;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

// ... (implementation added in Step 3) ...

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::{Catalog, ToolDef};

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
            upstreams: vec![UpstreamInfo { name: "github".into(), transport: "stdio".into() }],
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
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p dashboard api 2>&1 | tail -20`
Expected: FAIL —— `AppState`/`overview` 等未定义。

- [ ] **Step 3: 最小实现**

在 `crates/dashboard/src/api.rs` 测试模块**之前**插入：

```rust
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
        tools_total: snap.catalog.len(),
        total_calls: m.total_calls,
        last_rebuild_skipped: state.gateway.last_summary().map(|s| s.skipped.len()).unwrap_or(0),
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
            let tools = snap.catalog.iter().filter(|t| t.server == info.name).count();
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
    snap.catalog
        .iter()
        .filter(|t| match q {
            Some(needle) if !needle.is_empty() => {
                let n = needle.to_lowercase();
                t.qualified_name().to_lowercase().contains(&n)
                    || t.description.to_lowercase().contains(&n)
            }
            _ => true,
        })
        .map(|t| ToolView { name: t.qualified_name(), description: t.description.clone() })
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
                TracesResponse { source: "history".into(), history_unavailable: !ok, traces }
            }
            None => TracesResponse { source: "history".into(), history_unavailable: true, traces: Vec::new() },
        }
    } else {
        let traces = state.discovery.as_ref().map(|d| d.recent(limit)).unwrap_or_default();
        TracesResponse { source: "live".into(), history_unavailable: false, traces }
    }
}

pub fn metrics_history(state: &AppState, limit: usize, bucket_ms: u64) -> HistoryResponse {
    match &state.audit_path {
        Some(p) => {
            let (buckets, ok) = replay_audit_metrics(p, limit, bucket_ms);
            HistoryResponse { history_unavailable: !ok, buckets }
        }
        None => HistoryResponse { history_unavailable: true, buckets: Vec::new() },
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p dashboard`
Expected: PASS —— api 的 5 个测试 + 之前 metrics/trace/history 测试全绿。

- [ ] **Step 5: 加 axum 薄封装 + router（接前端在 Task 10）**

在 `crates/dashboard/src/lib.rs` 加：

```rust
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
async fn h_tools(State(s): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Json<Vec<api::ToolView>> {
    Json(api::tools(&s, q.get("q").map(|v| v.as_str())))
}
async fn h_metrics(State(s): State<Arc<AppState>>) -> Json<MetricsSnapshot> {
    Json(api::metrics(&s))
}
async fn h_traces(State(s): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Json<api::TracesResponse> {
    let limit = qparam_usize(&q, "limit", 100);
    let source = q.get("source").cloned().unwrap_or_else(|| "live".into());
    Json(api::traces(&s, limit, &source))
}
async fn h_metrics_history(State(s): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Json<api::HistoryResponse> {
    let limit = qparam_usize(&q, "limit", 5000);
    let bucket_ms = q.get("bucket_ms").and_then(|v| v.parse().ok()).unwrap_or(60_000u64);
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
```

确保 `lib.rs` 顶部 re-export 了 `MetricsSnapshot`（Task 5 已 `pub use`）。`UpstreamView`/`ToolView`/`Overview`/`TracesResponse`/`HistoryResponse` 在 `api` 模块内 `pub`，handler 以 `api::` 路径引用。

- [ ] **Step 6: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings && cargo test -p dashboard
git add crates/dashboard/src/api.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): read-only JSON API (overview/upstreams/tools/metrics/traces/history)

Pure per-endpoint logic over an AppState (gateway snapshot + MetricsSink +
DiscoveryRingSink + history files), with thin axum handlers and a router.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 10: dashboard — 静态前端 SPA（内嵌 + 静态路由）

**Files:**
- Create: `crates/dashboard/assets/index.html`、`assets/app.js`、`assets/style.css`
- Modify: `crates/dashboard/src/lib.rs`（`include_str!` 内嵌 + `/`、`/app.js`、`/style.css` 路由 + 资产非空测试）
- Test: `crates/dashboard/src/lib.rs` 的 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败测试**

在 `crates/dashboard/src/lib.rs` 末尾加：

```rust
#[cfg(test)]
mod asset_tests {
    use super::{APP_JS, INDEX_HTML, STYLE_CSS};

    #[test]
    fn embedded_assets_are_present_and_wired() {
        assert!(INDEX_HTML.contains("<title>"), "index has a title");
        assert!(INDEX_HTML.contains("app.js"), "index loads app.js");
        assert!(APP_JS.contains("/api/overview"), "app.js polls the API");
        assert!(STYLE_CSS.contains("{"), "style.css is non-empty CSS");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p dashboard embedded_assets 2>&1 | tail -20`
Expected: FAIL —— `INDEX_HTML` 等常量未定义 / 资产文件不存在（`include_str!` 编译错误）。

- [ ] **Step 3: 建静态资产**

新建 `crates/dashboard/assets/index.html`：

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>mcpgw dashboard</title>
  <link rel="stylesheet" href="/style.css" />
</head>
<body>
  <header><h1>mcpgw dashboard</h1><span id="uptime" class="muted"></span></header>
  <section id="overview" class="cards"></section>
  <section><h2>Upstreams</h2><table id="upstreams"><thead><tr>
    <th>name</th><th>transport</th><th>status</th><th>tools</th><th>calls</th><th>errors</th></tr></thead>
    <tbody></tbody></table></section>
  <section><h2>Meta-tool metrics</h2><div id="metrics"></div></section>
  <section><h2>Query traces</h2>
    <label>source:
      <select id="trace-source"><option value="live">live</option><option value="history">history</option></select>
    </label>
    <div id="traces"></div></section>
  <script src="/app.js"></script>
</body>
</html>
```

新建 `crates/dashboard/assets/style.css`：

```css
:root { font-family: system-ui, sans-serif; }
body { margin: 0 auto; max-width: 960px; padding: 1rem; color: #1c1c1c; }
header { display: flex; align-items: baseline; gap: 1rem; }
.muted { color: #777; font-size: .85rem; }
.cards { display: flex; flex-wrap: wrap; gap: .75rem; }
.card { border: 1px solid #ddd; border-radius: 8px; padding: .6rem .9rem; min-width: 120px; }
.card .v { font-size: 1.4rem; font-weight: 600; }
table { width: 100%; border-collapse: collapse; }
th, td { text-align: left; padding: .35rem .5rem; border-bottom: 1px solid #eee; }
.badge { padding: .1rem .5rem; border-radius: 10px; font-size: .8rem; }
.connected { background: #e3f6e3; } .skipped { background: #fde8e8; } .unknown { background: #eee; }
.bar { background: #eef; height: 14px; border-radius: 3px; }
.bar > span { display: block; height: 100%; background: #5b8def; border-radius: 3px; }
.trace { border-bottom: 1px solid #eee; padding: .4rem 0; }
.trace .q { font-weight: 600; } .hit { color: #444; font-size: .85rem; }
```

新建 `crates/dashboard/assets/app.js`：

```javascript
const REFRESH_MS = 3000;
const $ = (sel) => document.querySelector(sel);

async function j(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(url + " -> " + r.status);
  return r.json();
}

function card(label, value) {
  return `<div class="card"><div class="muted">${label}</div><div class="v">${value}</div></div>`;
}

async function refresh() {
  try {
    const ov = await j("/api/overview");
    $("#uptime").textContent = "up " + ov.uptime_secs + "s · strategy " + ov.strategy;
    $("#overview").innerHTML =
      card("upstreams", ov.upstreams_connected + "/" + ov.upstreams_total) +
      card("tools", ov.tools_total) +
      card("calls", ov.total_calls) +
      card("skipped", ov.last_rebuild_skipped);

    const ups = await j("/api/upstreams");
    $("#upstreams tbody").innerHTML = ups.map((u) =>
      `<tr><td>${u.name}</td><td>${u.transport}</td>` +
      `<td><span class="badge ${u.status}">${u.status}</span>${u.reason ? " " + u.reason : ""}</td>` +
      `<td>${u.tools}</td><td>${u.calls}</td><td>${u.errors}</td></tr>`).join("");

    const m = await j("/api/metrics");
    const maxCalls = Math.max(1, ...m.per_meta_tool.map((x) => x.calls));
    $("#metrics").innerHTML = m.per_meta_tool.map((x) =>
      `<div><b>${x.meta_tool}</b> calls ${x.calls} · err ${x.errors} · p50 ${x.p50_ms}ms · p95 ${x.p95_ms}ms` +
      `<div class="bar"><span style="width:${(100 * x.calls / maxCalls).toFixed(0)}%"></span></div></div>`).join("");

    const src = $("#trace-source").value;
    const t = await j("/api/traces?limit=50&source=" + src);
    $("#traces").innerHTML = t.history_unavailable
      ? `<p class="muted">history unavailable (enable [dashboard].trace_path)</p>`
      : t.traces.map((r) =>
          `<div class="trace"><div class="q">${escapeHtml(r.query)}</div>` +
          r.results.map((h) => `<span class="hit">${h.name} (${h.score.toFixed(2)})</span>`).join(" · ") +
          `</div>`).join("");
  } catch (e) {
    console.error(e);
  }
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

refresh();
setInterval(refresh, REFRESH_MS);
```

- [ ] **Step 4: 内嵌 + 静态路由**

在 `crates/dashboard/src/lib.rs` 顶部 `use` 区加：

```rust
use axum::http::header::CONTENT_TYPE;
use axum::response::{Html, IntoResponse};
```

在 `lib.rs` 模块级加常量与 handler：

```rust
const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLE_CSS: &str = include_str!("../assets/style.css");

async fn h_index() -> impl IntoResponse {
    Html(INDEX_HTML)
}
async fn h_app_js() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/javascript")], APP_JS)
}
async fn h_style_css() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/css")], STYLE_CSS)
}
```

在 `build_dashboard_router` 里追加三条静态路由（`.with_state(state)` 之前）：

```rust
        .route("/", get(h_index))
        .route("/app.js", get(h_app_js))
        .route("/style.css", get(h_style_css))
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p dashboard`
Expected: PASS —— `embedded_assets_are_present_and_wired` + 之前所有 dashboard 测试全绿。

- [ ] **Step 6: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings && cargo test -p dashboard
git add crates/dashboard/assets crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): static vanilla-JS SPA (overview/upstreams/metrics/traces)

Zero-build HTML/JS/CSS embedded via include_str!, served at / · /app.js ·
/style.css; polls the JSON API every 3s and renders cards/tables/CSS bars/traces.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 11: mcpgw 装配 — wire sinks + 起 dashboard 任务 + 优雅关停 + e2e

**Files:**
- Modify: `crates/mcpgw/Cargo.toml`（加 `dashboard` 依赖 + dev-dep `reqwest`）
- Modify: `crates/mcpgw/src/main.rs`（构造 MetricsSink/DiscoveryRingSink、注入下游、起 dashboard 任务、关停 drain）
- Create: `crates/mcpgw/tests/dashboard.rs`（e2e，`#[ignore]`，与现有 http/vector 真实冒烟同策）
- Test: `crates/mcpgw/tests/dashboard.rs`

背景：把前面各 crate 接到活的 `serve` 上。dashboard 关闭时零额外开销（不构造 sink、下游收空 discovery 切片）。

- [ ] **Step 1: 依赖**

`crates/mcpgw/Cargo.toml` 的 `[dependencies]` 加：

```toml
dashboard = { path = "../dashboard" }
```
`[dev-dependencies]` 加（若无）：

```toml
reqwest = { version = "0.13", default-features = false, features = ["rustls"] }
```

- [ ] **Step 2: main.rs —— 顶部常量**

在 `HTTP_SHUTDOWN_TIMEOUT` 常量附近加：

```rust
/// Upper bound on how long shutdown waits for the dashboard server to drain.
const DASHBOARD_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
```

并加一个 transport→字符串 helper（模块级，靠近其它 helper）：

```rust
fn transport_str(t: &UpstreamTransport) -> String {
    match t {
        UpstreamTransport::Stdio { .. } => "stdio".into(),
        UpstreamTransport::Http { .. } => "http".into(),
    }
}
```

- [ ] **Step 3: main.rs —— 构造 MetricsSink + DiscoveryRingSink（在 `let sinks: Arc<[...]> = sink_vec.into();` 之前）**

在 `audit_writer` 构造之后、`let sinks` 之前插入：

```rust
    // Dashboard's metrics sink (only when enabled) joins the CallSink fan-out.
    let dashboard_metrics = if cfg.dashboard.enabled {
        let m = Arc::new(dashboard::MetricsSink::new());
        sink_vec.push(m.clone() as Arc<dyn observe::CallSink>);
        Some(m)
    } else {
        None
    };
```

在 `let sinks: Arc<[Arc<dyn observe::CallSink>]> = sink_vec.into();` **之后**插入 discovery 构造：

```rust
    // Opt-in discovery capture (query -> tools). Ring buffer for live; optional JSONL for history.
    let (discovery_ring, discovery_writer) =
        if cfg.dashboard.enabled && cfg.dashboard.trace_queries {
            let (ring, writer) = dashboard::DiscoveryRingSink::spawn(
                cfg.dashboard.trace_buffer,
                cfg.dashboard.trace_path.as_deref().map(std::path::Path::new),
            )
            .map_err(|e| format!("open discovery trace file: {e}"))?;
            (Some(Arc::new(ring)), writer)
        } else {
            (None, None)
        };
    let discovery_sinks: Arc<[Arc<dyn observe::DiscoverySink>]> = match &discovery_ring {
        Some(r) => Arc::from(vec![r.clone() as Arc<dyn observe::DiscoverySink>]),
        None => Arc::from(Vec::new()),
    };
```

- [ ] **Step 4: main.rs —— 用真实 discovery 替换 Task 8 的空切片**

把 `downstream::http::build_router(...)` 调用（HTTP 装配处）末尾实参改为 `discovery_sinks.clone()`：

```rust
        let router = downstream::http::build_router(
            state.clone(),
            cfg.retrieval.top_k,
            &h.path,
            api_keys,
            sinks.clone(),
            discovery_sinks.clone(),
        );
```

把 stdio 臂里的 `downstream::GatewayServer::new(state_for_stdio, top_k, sinks.clone())` 改为：

```rust
            let server = downstream::GatewayServer::new(
                state_for_stdio,
                top_k,
                sinks.clone(),
                discovery_sinks.clone(),
            );
```

（删除 Task 8 临时加的 `no_discovery` 占位。）

- [ ] **Step 5: main.rs —— 绑定并起 dashboard 任务（在 http_task spawn 之后、`tokio::select!` 之前）**

```rust
    let (dash_shutdown_tx, dash_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let dashboard_enabled = cfg.dashboard.enabled;
    let mut dash_self_terminated = false;
    let mut dash_task = if dashboard_enabled {
        let listener = tokio::net::TcpListener::bind(&cfg.dashboard.bind)
            .await
            .map_err(|e| format!("bind dashboard {:?}: {e}", cfg.dashboard.bind))?;
        tracing::info!(bind = %cfg.dashboard.bind, "dashboard listening");
        if unauthenticated_public_bind(&cfg.dashboard.bind, false) {
            tracing::warn!(
                bind = %cfg.dashboard.bind,
                "dashboard is UNAUTHENTICATED and bound to a non-loopback address; bind to localhost"
            );
        }
        let app_state = Arc::new(dashboard::AppState {
            gateway: state.clone(),
            metrics: dashboard_metrics.clone().expect("metrics present when dashboard enabled"),
            discovery: discovery_ring.clone(),
            upstreams: cfg
                .upstreams
                .iter()
                .map(|u| dashboard::UpstreamInfo { name: u.name.clone(), transport: transport_str(&u.transport) })
                .collect(),
            strategy: cfg.retrieval.strategy.clone(),
            audit_path: cfg.audit.enabled.then(|| PathBuf::from(&cfg.audit.path)),
            discovery_path: cfg.dashboard.trace_path.as_ref().map(PathBuf::from),
            started_at: std::time::Instant::now(),
        });
        let router = dashboard::build_dashboard_router(app_state);
        Some(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = dash_shutdown_rx.await;
                })
                .await
                .map_err(|e| e.to_string())
        }))
    } else {
        None
    };
```

> 注：`axum` 需作为 `mcpgw` 的依赖之一（若 main.rs 尚未直接 `use`/依赖 axum，则在 `crates/mcpgw/Cargo.toml` 的 `[dependencies]` 加 `axum = { workspace = true }`；它已是 workspace 依赖）。

- [ ] **Step 6: main.rs —— select! 增 dashboard 臂**

在 `tokio::select!` 中、`ctrl_c` 臂之前插入：

```rust
        res = async {
            match dash_task.as_mut() {
                Some(t) => t.await.map_err(|e| e.to_string()).and_then(|r| r),
                None => std::future::pending().await,
            }
        }, if dashboard_enabled => {
            dash_self_terminated = true;
            res
        }
```

- [ ] **Step 7: main.rs —— 关停 drain（顺序：先关 dashboard，再放掉 discovery 句柄，最后 join writer）**

在 HTTP drain 块之后、`drop(sinks);` 之前插入 dashboard 关停：

```rust
    let _ = dash_shutdown_tx.send(());
    if !dash_self_terminated {
        if let Some(task) = dash_task {
            if tokio::time::timeout(DASHBOARD_SHUTDOWN_TIMEOUT, task).await.is_err() {
                tracing::warn!("dashboard graceful shutdown timed out");
            }
        }
    }
```

在 audit writer drain 之后（`for name in state.registry()...` 之前）加 discovery writer drain：

```rust
    // Release every DiscoveryRingSink clone (downstream sinks dropped with `sinks`, the dashboard
    // task already joined) so the writer's channel disconnects, then drain it.
    drop(discovery_sinks);
    drop(discovery_ring);
    if let Some(writer) = discovery_writer {
        if tokio::time::timeout(
            AUDIT_DRAIN_TIMEOUT,
            tokio::task::spawn_blocking(move || writer.join()),
        )
        .await
        .is_err()
        {
            tracing::warn!("discovery writer drain timed out; some traces may be unflushed");
        }
    }
```

> `drop(sinks)`（已有，line ~391）释放下游持有的 `Arc<dyn DiscoverySink>` 之外的 CallSink；下游 `GatewayServer` 自身在 stdio/http 任务结束时 drop，释放它持有的 discovery Arc。

- [ ] **Step 8: 编译 + 全量测试（默认套件）**

Run: `cargo build --all-targets && cargo test --all-features 2>&1 | grep "test result:"`
Expected: 全绿（dashboard e2e 是 `#[ignore]`，不在默认套件计数内）。

- [ ] **Step 9: e2e 测试（`#[ignore]`，显式运行）**

新建 `crates/mcpgw/tests/dashboard.rs`：

```rust
//! End-to-end: `mcpgw serve` with the dashboard enabled serves /api/* and captures a discovery
//! trace for a search_tools call. Ignored by default (binds a TCP port), run with `--ignored`.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::json;
use tokio::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpgw")
}

#[tokio::test]
#[ignore = "binds a TCP port; run with --ignored"]
async fn dashboard_serves_api_and_captures_a_trace() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let port = 20000 + (nanos % 20000) as u16;
    let cfg_path = std::env::temp_dir().join(format!("mcpgw-dash-{nanos}.toml"));
    {
        let mut f = std::fs::File::create(&cfg_path).unwrap();
        writeln!(
            f,
            "[server]\nstdio = true\n\n[dashboard]\nenabled = true\nbind = \"127.0.0.1:{port}\"\ntrace_queries = true\n"
        )
        .unwrap();
    }

    let (transport, _stderr) = TokioChildProcess::builder(Command::new(bin()).configure(|c| {
        c.arg("serve").arg("--config").arg(&cfg_path);
    }))
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let client = ().serve(transport).await.unwrap();

    // Drive a search so a discovery trace is captured (empty catalog -> empty results, still traced).
    let _ = client
        .call_tool(CallToolRequestParams {
            name: "search_tools".into(),
            arguments: json!({ "query": "weather forecast" }).as_object().cloned(),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();

    let ov: serde_json::Value = http.get(format!("{base}/api/overview")).send().await.unwrap().json().await.unwrap();
    assert_eq!(ov["strategy"], "bm25");

    let traces: serde_json::Value =
        http.get(format!("{base}/api/traces?source=live")).send().await.unwrap().json().await.unwrap();
    let arr = traces["traces"].as_array().unwrap();
    assert!(arr.iter().any(|t| t["query"] == "weather forecast"), "the search query was captured");

    client.cancel().await.unwrap();
    let _ = std::fs::remove_file(&cfg_path);
}
```

Run: `cargo test -p mcpgw --test dashboard -- --ignored`
Expected: PASS —— `/api/overview` 报 strategy=bm25，`/api/traces?source=live` 含该 query。

- [ ] **Step 10: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
git add crates/mcpgw/Cargo.toml crates/mcpgw/src/main.rs crates/mcpgw/tests/dashboard.rs
git commit -m "feat(mcpgw): wire the read-only dashboard into serve (subsystem A)

Build MetricsSink/DiscoveryRingSink when [dashboard].enabled, inject discovery
sinks into both downstreams, start the dashboard server as a separate task on its
own port with graceful shutdown + bounded writer drain. Adds an ignored e2e.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 12: 分层文档同步（L1–L4）+ 测试计数

**Files:**
- Create: `docs/L2-components/dashboard.md`、`docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`
- Modify: `docs/L1-overview.md`（新增 dashboard 段 + 测试计数 + 传输/可观测一览补充）
- Modify: `docs/L4-api/observe-lib.md`（补 `DiscoverySink`/`DiscoveryRecord` 契约）、`docs/L4-api/downstream-*.md`（`build_router`/`GatewayServer::new` 新增 discovery 参 + search 捕获）、`docs/L4-api/config-lib.md` 或 `docs/L3-details/config.md`（`[dashboard]` 段）、`docs/L2-components/metatools.md`/相关（`ToolSummary` 加 `score`）

文档为**中文**（含英文标识符）。先读各目标文件既有风格与对应源码，如实描述已落地代码。

- [ ] **Step 1: 新增 dashboard 三层文档**

按既有 L2/L3/L4 风格新建：
- `docs/L2-components/dashboard.md`（L2）：dashboard crate 职责、边界、依赖（`gateway`/`observe`/`catalog`/`config` + axum）、对外接口（`MetricsSink`/`DiscoveryRingSink`/`AppState`/`build_dashboard_router`），与核心 crate 的关系。
- `docs/L3-details/dashboard.md`（L3）：进程模型（独立任务/端口/panic 边界/有界缓冲/优雅关停顺序）；数据来源（快照 + MetricsSink + DiscoveryRingSink + 历史 JSONL 回放）；隐私（CallRecord 仍仅元数据，discovery 为独立 opt-in 通道）；MetricsSink 桶/百分位算法、DiscoveryRingSink ring+writer、history 限量回放。
- `docs/L4-api/dashboard.md`（L4）：逐符号 API——`MetricsSink`/`MetricsSnapshot`/`MetaToolMetrics`/`UpstreamMetrics`、`DiscoveryRingSink`/`DiscoveryWriter`、`replay_audit_metrics`/`replay_discovery`/`MetricBucket`、`AppState`/`UpstreamInfo`/各 view 类型/pure 函数、`build_dashboard_router` + 6 个 `/api/*` 端点 + 3 个静态路由。

- [ ] **Step 2: 更新相关既有 L4/L3 文档**

- `docs/L4-api/observe-lib.md`：补 `DiscoveryRecord { ts_unix_ms, query, top_k, results: Vec<DiscoveryHit{name,score}>, latency_ms }` + `DiscoverySink` trait（与 `CallSink`/`CallRecord` 并列，强调与仅元数据 `CallRecord` 隔离、opt-in）。
- `docs/L4-api/downstream-lib.md` + `docs/L4-api/downstream-http.md`：`GatewayServer::new` 与 `build_router` 新增 `discovery: Arc<[Arc<dyn observe::DiscoverySink>]>` 参（空=不捕获）；`search_tools` 分支在 discovery 非空时构造 `DiscoveryRecord` 扇出。
- `docs/L3-details/config.md`（及 `docs/L4-api/config-lib.md` 若涉及）：新增 `[dashboard]` 段（`enabled`/`bind`/`trace_queries`/`trace_path`/`trace_buffer`，默认值、`deny_unknown_fields`、`enabled` 时 `bind` 非空 + `trace_buffer>0` 校验）。
- `ToolSummary` 加 `score` 字段：更新提及 `ToolSummary` 的文档（如 `docs/L4-api/` 中 metatools/snapshot 相关、`docs/L3-details/downstream.md` 若有）。

- [ ] **Step 3: L1 概览**

在 `docs/L1-overview.md` 增一节「只读可视化面板（dashboard，子系统 A）」：简述能力（接入 MCP/状态、调用率/延迟、query→tools 追踪，实时+历史）、进程模型（独立端口、localhost、默认关）、与 observe 接缝的关系，并指向 L2/L3/L4。在「可观测性」叙述处点明 dashboard 复用 `CallSink` 接缝 + 新增 `DiscoverySink`。

- [ ] **Step 4: L1 测试计数**

Run: `cargo test --all-features 2>&1 | grep "test result:"`
把各套件 passed 求和，更新 `docs/L1-overview.md` 测试计数块：本计划新增约 +23 passed（observe +2、metatools +1、config +3、gateway +1、dashboard 15、downstream +1）→ 约 **221 passed**；ignored 由 3 → **4**（dashboard e2e）。**以实跑数为准**，逐套件分项要能求和。新增 dashboard 套件分项要列出。

- [ ] **Step 5: 校对 + 提交**

逐条核对每处文档与真实代码一致。
Run:
```bash
git add docs/
git commit -m "docs: layered docs for the read-only dashboard (L1-L4) + test count

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 13: 验证 + 最终审查 + 合并

**Files:** 无代码改动。

- [ ] **Step 1: 全门禁复跑**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features 2>&1 | grep "test result:"
cargo test -p mcpgw --test dashboard -- --ignored
cargo build --locked
```
Expected: fmt 干净、clippy 零告警、默认测试全绿（总数 = L1 所记）、ignored e2e 通过、build 成功。

- [ ] **Step 2: 整分支最终 code-review**

以 `git merge-base master <branch>` 为基，对整条分支 diff 跑 `code-review` 子代理（model `claude-opus-4.8`）。折叠 Critical/Important 项。

- [ ] **Step 3: 合并（finishing-a-development-branch）**

征得用户确认后，`--no-ff` 本地合并入 master、master 复跑 `cargo test --all-features` 全绿、删除分支。

- [ ] **Step 4: 推送 + 收尾**

`git push origin master`。向用户用中文汇报完成（含如何打开面板：`serve` 后浏览器访问 `http://127.0.0.1:8971`）。

---

## Self-Review（plan 作者自查）

- **Spec coverage**：架构/进程模型→Task 11；DiscoverySink 契约→Task 1；ToolSummary score→Task 2；`[dashboard]` 配置→Task 3；上游状态数据源（last_summary）→Task 4；MetricsSink→Task 5；DiscoveryRingSink+writer→Task 6；历史回放→Task 7；下游捕获→Task 8；JSON API→Task 9；前端 SPA→Task 10；装配+e2e→Task 11；文档+计数→Task 12；验证+合并→Task 13。隐私（CallRecord 仅元数据不变、discovery 独立 opt-in）贯穿 Task 1/8。范围外（写操作/鉴权/SSE/图表库）未越界。✓
- **Placeholder scan**：每个代码步骤含完整代码 + 确切命令；无 TBD/TODO。✓
- **Type/名一致**：`DiscoveryRecord`/`DiscoveryHit`/`DiscoverySink`、`MetricsSink`/`MetricsSnapshot`/`MetaToolMetrics`/`UpstreamMetrics`、`DiscoveryRingSink`/`DiscoveryWriter`、`replay_audit_metrics`/`replay_discovery`/`MetricBucket`、`AppState`/`UpstreamInfo`、`build_dashboard_router`、`discovery_record_for_search`、`GatewayState::last_summary`、`ToolSummary.score` 全程一致。`GatewayServer::new`/`build_router` 的新 `discovery` 参在 Task 8 引入、Task 11 注入真实值，所有调用处（含 tests、main.rs）在 Task 8 一并更新以保持每步绿。✓
