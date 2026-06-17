# M1：逐条调用数据层（Call Records Data Layer）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 dashboard 下钻提供逐条调用记录的后端数据源：一个有界内存环 `CallRingSink` + audit JSONL 历史回放，并暴露 `GET /api/calls` 列表与 `GET /api/calls/{id}` 详情两个只读端点。

**Architecture:** 新增 `CallRingSink`（实现已有 `observe::CallSink` 接缝，加入 main.rs 的 sink 扇出，仅 `[dashboard].enabled` 时挂载），内部 `Mutex<VecDeque<StoredCall>>` + `AtomicU64` 单调 seq，满则淘汰最旧——与现有 `DiscoveryRingSink` 同构。历史源复用 `history.rs` 的 `tail_lines` 把 audit JSONL 反序列化为统一的 owned `CallItem`。API 层（`api.rs` 纯函数 + `lib.rs` handler）按 `source=live|history` 取数、过滤、分页。

**Tech Stack:** Rust、axum 0.8（路径参数 `{id}`）、serde/serde_json、`std::sync::{Mutex, atomic::AtomicU64}`、`std::collections::VecDeque`。

**关键约束（来自现状，务必遵守）：**
- `observe::CallRecord` 只派生 `Serialize`、且 `error_kind: Option<&'static str>`，**无法直接反序列化**。因此历史回放必须用一个 owned 镜像结构 `AuditCallLine`（`error_kind: Option<String>`），再映射到统一的 `CallItem`。
- Mutex 上锁一律用 `.lock().unwrap_or_else(|e| e.into_inner())`（dashboard crate 既有约定，见 `metrics.rs:110`、`trace.rs:67`），自愈毒化、不 panic。
- `CallSink::record` 必须非阻塞、不 panic（trait 契约，`observe/src/lib.rs:86-90`）。
- 所有缓冲有界：环满 `pop_front` 淘汰最旧（与 `trace.rs:81-84` 一致）；历史扫描 `limit` 夹紧到 `MAX_HISTORY_LIMIT`。

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/config/src/lib.rs` | `DashboardConfig` 加 `call_buffer` 字段 + 默认值 + 校验 | 修改 |
| `crates/dashboard/src/calls.rs` | `CallItem`/`CallFilter`/`StoredCall`/`CallRingSink`（环+seq+查询+取单条） | 新建 |
| `crates/dashboard/src/history.rs` | 新增 `AuditCallLine` + `replay_audit_calls`（复用 `tail_lines`） | 修改 |
| `crates/dashboard/src/api.rs` | `CallsResponse` + `calls()`/`call_detail()`/`call_filter_from_query()` | 修改 |
| `crates/dashboard/src/lib.rs` | `mod calls;` + 导出 + `h_calls`/`h_call_detail` handler + 两条路由 | 修改 |
| `crates/mcpgw/src/main.rs` | 构建 `CallRingSink`、加入 sink 扇出、注入 `AppState.calls` | 修改 |
| `docs/L1-overview.md` / `docs/L3-details/dashboard.md` / `docs/L4-api/dashboard.md` | 同步分层文档 | 修改 |

---

## Task 1：DashboardConfig 增加 `call_buffer` 字段

**Files:**
- Modify: `crates/config/src/lib.rs:153-176`（`DashboardConfig` 结构体 + `Default`）
- Modify: `crates/config/src/lib.rs:370-373` 附近（`validate` 中 `trace_buffer` 校验旁）
- Test: `crates/config/src/lib.rs`（同文件 `#[cfg(test)]`，镜像 `dashboard_defaults_and_partial_fill`）

- [ ] **Step 1: 写失败测试**

在 `crates/config/src/lib.rs` 的测试模块中，找到 `fn dashboard_defaults_and_partial_fill()` 测试，在其断言中追加对 `call_buffer` 默认值的断言；并新增一个拒绝 `call_buffer=0` 的测试：

```rust
#[test]
fn dashboard_call_buffer_defaults_to_2000() {
    let cfg = Config::from_toml_str("").unwrap();
    assert_eq!(cfg.dashboard.call_buffer, 2000);
}

#[test]
fn dashboard_call_buffer_zero_is_rejected() {
    let cfg = Config::from_toml_str("[dashboard]\nenabled = true\ncall_buffer = 0\n").unwrap();
    let err = cfg.validate().expect_err("call_buffer=0 must be rejected");
    assert!(
        err.to_string().contains("call_buffer"),
        "error should mention call_buffer, got: {err}"
    );
}
```

> 同文件既有约定：用 `Config::from_toml_str(...)` 解析、`validate()` 返回 `Result<(), ConfigError>`（错误变体 `ConfigError::Invalid(String)`，见 `lib.rs:366,371`）。`dashboard_defaults_and_partial_fill`（`lib.rs:815`）也可顺手补一行 `assert_eq!(cfg.dashboard.call_buffer, 2000);`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p config dashboard_call_buffer`
Expected: FAIL —— 编译错误 `no field call_buffer on type DashboardConfig`。

- [ ] **Step 3: 加字段 + 默认值 + 校验**

在 `DashboardConfig` 结构体里、`trace_buffer` 字段之后追加：

```rust
    /// In-memory per-call ring buffer size (drives the Calls drill-down list). Must be > 0.
    pub call_buffer: usize,
```

在 `impl Default for DashboardConfig` 的 `trace_buffer: 500,` 之后追加：

```rust
            call_buffer: 2000,
```

在 `validate()` 中现有 `trace_buffer == 0` 校验之后（仍在 `if self.dashboard.enabled` 块内，`lib.rs:374` 那个 `}` 之前）追加同构校验：

```rust
            if self.dashboard.call_buffer == 0 {
                return Err(ConfigError::Invalid(
                    "[dashboard].call_buffer must be > 0".into(),
                ));
            }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p config dashboard_call_buffer`
Expected: PASS（两个新测试都过）。

- [ ] **Step 5: 跑全 crate 测试 + fmt**

Run: `cargo test -p config && cargo fmt -p config --check`
Expected: 全过；fmt 无 diff。

- [ ] **Step 6: 提交**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): add [dashboard].call_buffer (default 2000, must be > 0)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：`calls.rs` —— `CallItem` / `CallFilter` / `CallRingSink`

**Files:**
- Create: `crates/dashboard/src/calls.rs`
- Test: `crates/dashboard/src/calls.rs`（同文件 `#[cfg(test)]`，镜像 `trace.rs` 的测试风格）

本任务建立逐条调用的统一 owned 类型 `CallItem`、过滤器 `CallFilter`、内部环条目 `StoredCall`，以及实现 `observe::CallSink` 的有界环 `CallRingSink`。

- [ ] **Step 1: 写失败测试**

新建 `crates/dashboard/src/calls.rs`，先只放测试（让它因缺类型而编译失败）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use observe::{CallOutcome, CallRecord, CallSink, MetaTool};

    fn rec(meta: MetaTool, upstream: Option<&str>, tool: Option<&str>, outcome: CallOutcome, ts: u64) -> CallRecord {
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
        ring.record(&rec(MetaTool::CallTool, Some("a"), Some("a__t"), CallOutcome::Ok, 1));
        ring.record(&rec(MetaTool::CallTool, Some("b"), Some("b__t"), CallOutcome::Ok, 2));
        ring.record(&rec(MetaTool::CallTool, Some("c"), Some("c__t"), CallOutcome::Ok, 3)); // evicts first
        let (items, total) = ring.query(&CallFilter::default(), 10, 0);
        assert_eq!(total, 2, "capacity 2");
        let ups: Vec<_> = items.iter().map(|i| i.upstream.as_deref().unwrap()).collect();
        assert_eq!(ups, ["c", "b"], "newest first");
    }

    #[test]
    fn seq_is_monotonic_and_get_resolves_live_id() {
        let ring = CallRingSink::new(10);
        ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 1));
        ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 2));
        let (items, _) = ring.query(&CallFilter::default(), 10, 0);
        // newest-first: ids are "1" then "0"
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].id, "0");
        let one = ring.get(1).expect("seq 1 present");
        assert_eq!(one.ts_unix_ms, 2);
        assert!(ring.get(999).is_none(), "absent seq -> None");
    }

    #[test]
    fn filter_by_each_dimension() {
        let ring = CallRingSink::new(10);
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__issue"), CallOutcome::Error, 10));
        ring.record(&rec(MetaTool::SearchTools, Some("gh"), None, CallOutcome::Ok, 20));
        ring.record(&rec(MetaTool::CallTool, Some("wx"), Some("wx__now"), CallOutcome::Ok, 30));

        let f = CallFilter { meta_tool: Some("call_tool".into()), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 2);

        let f = CallFilter { upstream: Some("gh".into()), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 2);

        let f = CallFilter { target_tool: Some("wx__now".into()), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 1);

        let f = CallFilter { outcome: Some("error".into()), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 1);

        let f = CallFilter { since_ms: Some(20), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 2);

        let f = CallFilter { until_ms: Some(20), ..Default::default() };
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
        // newest-first ts = [4,3,2,1,0]; offset 1 limit 2 -> [3,2]
        assert_eq!(page[0].ts_unix_ms, 3);
        assert_eq!(page[1].ts_unix_ms, 2);
    }

    #[test]
    fn empty_ring_and_limit_zero_and_offset_overflow() {
        let ring = CallRingSink::new(4);
        assert_eq!(ring.query(&CallFilter::default(), 10, 0), (vec![], 0));
        ring.record(&rec(MetaTool::CallTool, None, None, CallOutcome::Ok, 1));
        assert_eq!(ring.query(&CallFilter::default(), 0, 0).0.len(), 0, "limit 0 -> empty page");
        assert_eq!(ring.query(&CallFilter::default(), 10, 99).0.len(), 0, "offset past end -> empty page");
        assert_eq!(ring.query(&CallFilter::default(), 10, 99).1, 1, "...but total still 1");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard calls::`
Expected: FAIL —— 编译错误 `cannot find type CallRingSink/CallFilter/CallItem`。

- [ ] **Step 3: 写实现（放在测试模块之上）**

```rust
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
    pub fn query(&self, filter: &CallFilter, limit: usize, offset: usize) -> (Vec<CallItem>, usize) {
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
```

- [ ] **Step 4: 注册模块（让测试可编译）**

在 `crates/dashboard/src/lib.rs` 顶部模块声明区（`mod trace;` 附近）加：

```rust
mod calls;
pub use calls::{CallFilter, CallItem, CallRingSink};
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p dashboard calls::`
Expected: PASS（5 个测试全过）。

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt -p dashboard --check && cargo clippy -p dashboard --all-targets -- -D warnings`
Expected: 无 diff、无 warning。

- [ ] **Step 7: 提交**

```bash
git add crates/dashboard/src/calls.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): add CallRingSink bounded per-call ring + CallItem/CallFilter

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：`history.rs` —— `replay_audit_calls`（audit JSONL → `CallItem`）

**Files:**
- Modify: `crates/dashboard/src/history.rs`（复用既有 `tail_lines`，新增 `AuditCallLine` + `replay_audit_calls`）
- Test: `crates/dashboard/src/history.rs`（同文件 `#[cfg(test)]`，镜像既有 `replay_audit_*` 测试）

`CallRecord` 不可反序列化（`error_kind: &'static str` + 只派生 `Serialize`），故用 owned 镜像 `AuditCallLine` 解析每行，映射为统一 `CallItem`，并分配稳定的历史 id `"h{ts}-{n}"`（`n` = 文件顺序中同一 `ts` 的第几条）。

- [ ] **Step 1: 写失败测试**

在 `crates/dashboard/src/history.rs` 测试模块末尾追加：

```rust
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
        // newest-first by file order: ts=2 line, then the two ts=1 lines (reversed)
        let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["h2-0", "h1-1", "h1-0"], "ids stable: n counts same-ts in file order");
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
        let f = crate::calls::CallFilter { outcome: Some("error".into()), ..Default::default() };
        let (items, ok) = replay_audit_calls(&p, 10, &f);
        assert!(ok);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].upstream.as_deref(), Some("wx"));
        let _ = std::fs::remove_file(&p);
    }
```

> `write(...)` 是 history.rs 测试模块里**已有**的 helper（`crates/dashboard/src/history.rs:92`）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard history::tests::replay_audit_calls`
Expected: FAIL —— `cannot find function replay_audit_calls`。

- [ ] **Step 3: 写实现**

在 `crates/dashboard/src/history.rs` 顶部 `use` 区补 import（已有 `use std::collections::{BTreeMap, VecDeque};`，无需重复）：

```rust
use crate::calls::{CallFilter, CallItem};
```

在文件中（`replay_audit_metrics` 之后、`#[cfg(test)]` 之前）新增：

```rust
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
            };
            if filter.matches(&item) {
                items.push(item);
            }
        }
    }
    items.reverse(); // file order is oldest-first -> newest-first
    (items, true)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dashboard history::tests::replay_audit_calls`
Expected: PASS（3 个新测试全过）。

- [ ] **Step 5: 全 crate 测试 + fmt + clippy**

Run: `cargo test -p dashboard && cargo fmt -p dashboard --check && cargo clippy -p dashboard --all-targets -- -D warnings`
Expected: 全过、无 diff、无 warning。

- [ ] **Step 6: 提交**

```bash
git add crates/dashboard/src/history.rs
git commit -m "feat(dashboard): replay_audit_calls — audit JSONL history -> CallItem (stable ids)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：接线 —— `AppState.calls` + main.rs sink 扇出

**Files:**
- Modify: `crates/dashboard/src/api.rs:19-28`（`AppState` 加 `calls` 字段）
- Modify: `crates/mcpgw/src/main.rs:306-314`（构建 `CallRingSink`、加入 `sink_vec`）
- Modify: `crates/mcpgw/src/main.rs:408-426`（`AppState` 构造处注入 `calls`）

本任务把环挂到 sink 扇出，并让 dashboard 的 `AppState` 持有它。**纯接线，无新单测**；正确性由 Task 5 的 handler 测试 + 现有 `run_serve` 冒烟覆盖。

- [ ] **Step 1: `AppState` 加字段（含更新测试 helper）**

`crates/dashboard/src/api.rs` 的 `pub struct AppState` 中，在 `pub discovery: Option<Arc<DiscoveryRingSink>>,` 之后追加：

```rust
    /// Per-call ring for the Calls drill-down (present only when the dashboard is enabled).
    pub calls: Option<Arc<crate::calls::CallRingSink>>,
```

**同时**，本文件测试模块里的 `seeded_state()`（`api.rs:204` 附近）构造 `AppState { ... }` 处，在 `discovery: None,` 之后补一行，否则该 helper 编译失败：

```rust
            calls: None,
```

- [ ] **Step 2: main.rs 构建环并加入扇出**

`crates/mcpgw/src/main.rs` 中，紧接现有 `dashboard_metrics` 构建块（`let dashboard_metrics = ...; ` 整段之后、`let sinks: Arc<[...]> = sink_vec.into();` 之前）插入：

```rust
    // Per-call ring for the dashboard Calls drill-down (only when dashboard enabled). Joins the
    // CallSink fan-out alongside MetricsSink; bounded by [dashboard].call_buffer.
    let dashboard_calls = if cfg.dashboard.enabled {
        let c = Arc::new(dashboard::CallRingSink::new(cfg.dashboard.call_buffer));
        sink_vec.push(c.clone() as Arc<dyn observe::CallSink>);
        Some(c)
    } else {
        None
    };
```

- [ ] **Step 3: 注入 `AppState`**

`crates/mcpgw/src/main.rs` 中构造 `dashboard::AppState { ... }` 处，在 `discovery: discovery_ring.clone(),` 之后追加：

```rust
            calls: dashboard_calls.clone(),
```

- [ ] **Step 4: 构建确认通过**

Run: `cargo build -p mcpgw`
Expected: 编译通过（无 unused warning：`dashboard_calls` 在 AppState 构造处被消费）。

- [ ] **Step 5: 既有测试仍过（dashboard + mcpgw）**

Run: `cargo test -p dashboard -p mcpgw`
Expected: PASS —— dashboard 的 `seeded_state()` 已补 `calls: None` 故编译通过；含 `run_serve_builds_initial_snapshot_with_no_upstreams`，确认接线不破坏启动。

- [ ] **Step 6: fmt + 提交**

Run: `cargo fmt -p dashboard -p mcpgw --check`
Expected: 无 diff。

```bash
git add crates/dashboard/src/api.rs crates/mcpgw/src/main.rs
git commit -m "feat(dashboard): wire CallRingSink into sink fan-out + AppState

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5：API 端点 —— `/api/calls` 列表 + `/api/calls/{id}` 详情

**Files:**
- Modify: `crates/dashboard/src/api.rs`（`use` 补 `replay_audit_calls`；新增 `CallsResponse`、`CALL_HISTORY_DETAIL_SCAN`、`call_filter_from_query`、`calls`、`call_detail`；测试）
- Modify: `crates/dashboard/src/lib.rs`（`h_calls`/`h_call_detail` handler + 两条路由）

源（live 环 / history 回放）由 `source=live|history` 选择；过滤、分页统一在 API 层完成。详情按 id 前缀路由：`h...` 走历史回放扫描，否则按 live seq 取环。

- [ ] **Step 1: 写失败测试（api.rs 纯函数）**

在 `crates/dashboard/src/api.rs` 测试模块末尾追加（复用既有 `seeded_state()` + 结构体更新语法注入 ring / audit_path）：

```rust
    fn call_rec(meta: observe::MetaTool, upstream: Option<&str>, outcome: observe::CallOutcome, ts: u64) -> observe::CallRecord {
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
        ring.record(&call_rec(observe::MetaTool::CallTool, Some("gh"), observe::CallOutcome::Ok, 1));
        ring.record(&call_rec(observe::MetaTool::CallTool, Some("wx"), observe::CallOutcome::Error, 2));
        ring.record(&call_rec(observe::MetaTool::SearchTools, Some("gh"), observe::CallOutcome::Ok, 3));
        let st = AppState { calls: Some(ring), ..seeded_state().await };

        let f = crate::calls::CallFilter { meta_tool: Some("call_tool".into()), ..Default::default() };
        let resp = calls(&st, &f, "live", CALL_HISTORY_DETAIL_SCAN, 100, 0);
        assert_eq!(resp.source, "live");
        assert!(!resp.history_unavailable);
        assert_eq!(resp.total, 2, "two call_tool calls");
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.items[0].upstream.as_deref(), Some("wx"), "newest-first");
    }

    #[tokio::test]
    async fn calls_live_empty_when_no_ring() {
        let st = seeded_state().await; // calls: None
        let resp = calls(&st, &crate::calls::CallFilter::default(), "live", CALL_HISTORY_DETAIL_SCAN, 100, 0);
        assert_eq!(resp.total, 0);
        assert!(resp.items.is_empty());
        assert!(!resp.history_unavailable);
    }

    #[tokio::test]
    async fn calls_history_unavailable_without_audit_path() {
        let st = seeded_state().await; // audit_path: None
        let resp = calls(&st, &crate::calls::CallFilter::default(), "history", CALL_HISTORY_DETAIL_SCAN, 100, 0);
        assert_eq!(resp.source, "history");
        assert!(resp.history_unavailable);
        assert!(resp.items.is_empty());
    }

    #[tokio::test]
    async fn call_detail_live_by_seq_and_404() {
        let ring = Arc::new(crate::calls::CallRingSink::new(10));
        ring.record(&call_rec(observe::MetaTool::CallTool, Some("gh"), observe::CallOutcome::Ok, 7));
        let st = AppState { calls: Some(ring), ..seeded_state().await };
        let item = call_detail(&st, "0").expect("seq 0 present");
        assert_eq!(item.ts_unix_ms, 7);
        assert!(call_detail(&st, "999").is_none(), "absent seq -> None");
        assert!(call_detail(&st, "not-a-number").is_none(), "garbage id -> None");
    }

    #[tokio::test]
    async fn call_detail_history_by_composite_id() {
        let body = "{\"ts_unix_ms\":5,\"meta_tool\":\"call_tool\",\"upstream\":\"gh\",\"latency_ms\":1,\"outcome\":\"ok\",\"arg_bytes\":0,\"result_bytes\":0}\n";
        let p = std::env::temp_dir().join(format!("mcpgw-detail-{}.jsonl", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let st = AppState { audit_path: Some(p.clone()), ..seeded_state().await };
        let item = call_detail(&st, "h5-0").expect("history id resolves");
        assert_eq!(item.ts_unix_ms, 5);
        assert_eq!(item.upstream.as_deref(), Some("gh"));
        assert!(call_detail(&st, "h5-9").is_none(), "absent history id -> None");
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard api::tests::call`
Expected: FAIL —— `cannot find function calls/call_detail/call_filter_from_query`、`CALL_HISTORY_DETAIL_SCAN`、`CallsResponse`。

- [ ] **Step 3: 写实现（api.rs）**

在 `crates/dashboard/src/api.rs` 顶部把 history import 改为含 `replay_audit_calls`：

```rust
use crate::history::{replay_audit_calls, replay_audit_metrics, replay_discovery, MetricBucket};
```

在 `HistoryResponse` 之后（其它 `#[derive(Serialize)]` 视图旁）新增类型与常量：

```rust
/// Max audit lines scanned when resolving a single history call id (`/api/calls/{id}`).
pub const CALL_HISTORY_DETAIL_SCAN: usize = 50_000;

#[derive(Serialize)]
pub struct CallsResponse {
    pub source: String,
    pub history_unavailable: bool,
    pub total: usize,
    pub items: Vec<crate::calls::CallItem>,
}
```

在文件的纯函数区（`metrics_history` 之后）新增：

```rust
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

/// Resolve one call id: `h...` -> history (re-scan + find), else decimal seq -> live ring. `None`
/// if not found / source unavailable.
pub fn call_detail(state: &AppState, id: &str) -> Option<crate::calls::CallItem> {
    if id.starts_with('h') {
        let p = state.audit_path.as_ref()?;
        let (items, ok) = replay_audit_calls(p, CALL_HISTORY_DETAIL_SCAN, &crate::calls::CallFilter::default());
        if !ok {
            return None;
        }
        items.into_iter().find(|c| c.id == id)
    } else {
        let seq: u64 = id.parse().ok()?;
        state.calls.as_ref()?.get(seq)
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dashboard api::tests::call`
Expected: PASS（6 个新测试全过）。

- [ ] **Step 5: 写实现（lib.rs handler + 路由）**

在 `crates/dashboard/src/lib.rs` 顶部 `use` 区补：

```rust
use axum::extract::Path;
```

在 handler 区（`h_metrics_history` 之后）新增：

```rust
async fn h_calls(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<api::CallsResponse> {
    let filter = api::call_filter_from_query(&q);
    let source = q.get("source").cloned().unwrap_or_else(|| "live".into());
    let limit = qparam_usize(&q, "limit", 100).min(MAX_HISTORY_LIMIT);
    let offset = qparam_usize(&q, "offset", 0);
    // History reads a JSONL file off the blocking pool (see h_traces); live reads the in-memory ring.
    if source == "history" {
        let resp = tokio::task::spawn_blocking(move || {
            api::calls(&s, &filter, &source, MAX_HISTORY_LIMIT, limit, offset)
        })
        .await
        .expect("calls history replay task");
        Json(resp)
    } else {
        Json(api::calls(&s, &filter, &source, MAX_HISTORY_LIMIT, limit, offset))
    }
}

async fn h_call_detail(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if id.starts_with('h') {
        let detail = tokio::task::spawn_blocking(move || api::call_detail(&s, &id))
            .await
            .expect("call detail replay task");
        match detail {
            Some(item) => Json(item).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        match api::call_detail(&s, &id) {
            Some(item) => Json(item).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}
```

在 `build_dashboard_router` 的路由链里（`/api/metrics/history` 之后）加两条：

```rust
        .route("/api/calls", get(h_calls))
        .route("/api/calls/{id}", get(h_call_detail))
```

- [ ] **Step 6: 全量验证**

Run: `cargo test -p dashboard && cargo fmt -p dashboard --check && cargo clippy -p dashboard --all-targets -- -D warnings`
Expected: 全过、无 diff、无 warning。

- [ ] **Step 7: 提交**

```bash
git add crates/dashboard/src/api.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): GET /api/calls list + /api/calls/{id} detail (live + history)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6：分层文档同步 + M1 整体门禁验证

**Files:**
- Modify: `docs/L4-api/dashboard.md`（新增 `calls.rs` 段、`history.rs` 增 `replay_audit_calls`、`api.rs` 增 `CallsResponse`/`calls`/`call_detail`/`call_filter_from_query`、`AppState` 增 `calls` 字段、路由表加 2 条、端点计数 6→8）
- Modify: `docs/L3-details/dashboard.md`（「数据来源」补逐条调用环；「测试覆盖」计数）
- Modify: `docs/L1-overview.md`（测试计数；M 路线图补「M1 逐条调用数据层」一句）

> 文档须忠实反映代码（用户偏好 L1-L4 分层、与代码同提交）。本任务**不改代码**，无需 TDD；最后跑全门禁确认 M1 整体绿。

- [ ] **Step 1: 更新 L4（`docs/L4-api/dashboard.md`）**

1）开头那句「聚合成 **6 个** `/api/*` 端点」改为「**8 个**」。
2）在 `## history.rs：JSONL 历史回放` 段，`replay_audit_metrics` 之后新增：

```markdown
### `replay_audit_calls`
```rust
pub fn replay_audit_calls(path: &Path, scan_limit: usize, filter: &CallFilter) -> (Vec<CallItem>, bool)
```
把 audit JSONL 反序列化为 `CallItem`（owned 镜像 `AuditCallLine`，因 `CallRecord` 不可反序列化），最新优先，扫描至多末尾 `scan_limit` 行；坏行跳过；id 为 `"h{ts}-{n}"`（同 `ts` 文件序内第 n 条，稳定）；`filter` 在 id 分配后应用。Bool=文件可读。
```

3）新增一整段（放在 `## history.rs` 之后、`## api.rs` 之前）：

```markdown
## `calls.rs`：逐条调用环 + 统一项类型

### `struct CallItem`（`Serialize`）
live 环与 history 回放共用的 owned 项：`id`（live=十进制 seq；history=`"h{ts}-{n}"`）、`ts_unix_ms`、`meta_tool`、`target_tool?`、`upstream?`、`latency_ms`、`outcome`、`error_kind?`、`arg_bytes`、`result_bytes`。仅元数据。

### `struct CallFilter`
`meta_tool`/`upstream`/`target_tool`/`outcome`/`since_ms`/`until_ms`，均 `Option`（`None`=全匹配）；`matches(&CallItem)` 对两数据源统一过滤。

### `struct CallRingSink`（实现 `observe::CallSink`）
有界内存环（满淘汰最旧，镜像 `DiscoveryRingSink`），每条插入分配单调 `seq` 作 live id。`query(&CallFilter, limit, offset) -> (Vec<CallItem>, total)` 最新优先、`total` 计全部命中；`get(seq) -> Option<CallItem>`。容量 = `[dashboard].call_buffer`。
```

4）`### struct AppState` 的字段块补一行 `calls: Option<Arc<CallRingSink>>`（present only when dashboard enabled）。
5）`### 视图类型` 补 `CallsResponse { source, history_unavailable, total, items: Vec<CallItem> }`。
6）`### 纯函数` 补 `call_filter_from_query`、`calls`、`call_detail`（含 `CALL_HISTORY_DETAIL_SCAN=50_000`）。
7）`build_dashboard_router` 路由表新增两行：

```markdown
| GET | `/api/calls?source=live\|history&meta=&upstream=&tool=&outcome=&since=&until=&limit=&offset=` | `Json<CallsResponse>`（`limit` 缺省 100、`min(50_000)`；`offset` 缺省 0；`source` 缺省 `"live"`） |
| GET | `/api/calls/{id}` | `Json<CallItem>` 或 404（`h…`→历史回放定位；否则按 live seq 取环） |
```

- [ ] **Step 2: 更新 L3（`docs/L3-details/dashboard.md`）**

在 `## 数据来源` 段补一条：逐条调用走新增的 `CallRingSink`（内存环，`[dashboard].call_buffer` 上界，满淘汰最旧）+ 可选 audit JSONL 历史回放（`replay_audit_calls`），与 Traces 的「实时环 + 历史回放」双源模型一致；`/api/calls` 列表、`/api/calls/{id}` 详情。在 `## 测试覆盖` 计数处加上本里程碑新增的单测（calls.rs 5 + history.rs 3 + api.rs 6 + config 2）。

- [ ] **Step 3: 更新 L1（`docs/L1-overview.md`）**

1）`## 构建与测试` 的测试计数行，用 **Step 5 实测的数字**替换（M1 预计 +16：config +2、dashboard calls +5 / history +3 / api +6）。
2）M 路线图区补一句：`M1（逐条调用数据层）✅ —— CallRingSink 内存环 + audit JSONL 历史回放，支撑 dashboard Calls 下钻；新增 /api/calls 与 /api/calls/{id}`。

- [ ] **Step 4: M1 整体门禁（硬门）**

Run（逐条，全绿才算 M1 完成）：

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```

Expected: fmt 无 diff；clippy 无 warning；`cargo test --all-features` 全过（记下 `N passed / 4 ignored` 的 N，回填 L1）；build 成功。

- [ ] **Step 5: 回填实测计数并提交**

把 Step 4 实测的 `passed` 数填入 `docs/L1-overview.md` 后：

```bash
git add docs/L1-overview.md docs/L3-details/dashboard.md docs/L4-api/dashboard.md
git commit -m "docs: sync L1/L3/L4 for M1 per-call data layer (CallRingSink + /api/calls)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## M1 完成判据（Definition of Done）

- [ ] `[dashboard].call_buffer` 配置项落地（默认 2000，`=0` 被 `validate` 拒绝）。
- [ ] `CallRingSink` 有界、最新优先、seq 稳定、各维度过滤 + 分页正确；满淘汰最旧。
- [ ] `replay_audit_calls` 历史回放：缺文件降级、坏行跳过、最新优先、id 稳定、过滤生效。
- [ ] `CallRingSink` 已挂入 sink 扇出，`AppState.calls` 注入。
- [ ] `GET /api/calls`（live/history、过滤、分页）与 `GET /api/calls/{id}`（live seq / history 复合 id / 404）可用。
- [ ] L1/L3/L4 文档与代码一致，端点计数更新为 8、测试计数回填。
- [ ] 四道门禁（fmt/clippy/test --all-features/build --locked）全绿。

## 给实现者的备注

- **DRY**：`CallItem` 是 live 与 history **唯一**的对外项类型，过滤只有 `CallFilter::matches` 一处；勿为两数据源各写一套。
- **YAGNI**：M1 只做数据层 + 两个只读端点，**不**碰前端（M2）、**不**做禁用/改配（M4/M5）。
- **隐私**：`CallItem` 只有元数据；不要把参数/结果内容引入任何字段。
- **锁纪律**：环的 `Mutex` 锁内不得 `.await`；一律 `.lock().unwrap_or_else(|e| e.into_inner())`。
- **阻塞池**：history 路径读文件，handler 里用 `spawn_blocking`（与 `h_traces`/`h_metrics_history` 一致），勿在 async 里同步读盘。
