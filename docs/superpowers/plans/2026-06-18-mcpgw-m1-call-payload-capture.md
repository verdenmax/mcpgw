# M1：调用内容捕获 + 详情展示 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 每次 meta-tool 调用把 **arguments + result（含上游错误文本）** 捕获进面板内存环（单条截断），并在 Call detail（`/api/calls/{id}`）展示；列表（`/api/calls`）保持轻、不含内容。元数据 `CallRecord` → tracing/audit/metrics **完全不变**。

**Architecture:** 新增 `observe::CallContent` + `CallContentSink`（镜像 `DiscoverySink`）。`downstream::call_tool` 在现有仅元数据 `CallRecord` 扇出之外，额外构造截断后的 `CallContent` 扇出到 `content_sinks`（仅面板启用时）。面板的 `CallRingSink` 从 `impl CallSink` 改为 `impl CallContentSink`，每条 call 自带内容；`CallItem` 列表投影省略内容、详情投影含内容。

**Tech Stack:** Rust（observe/downstream/dashboard/config/mcpgw crates）、Svelte 5 + Vite（CallDetail 展示 + dist 入库）。

**关键约束：**
- **元数据路径不动**：`CallRecord` 仍仅元数据，继续喂 `sinks`（TracingSink/JsonlSink/MetricsSink）；既有「9 键仅元数据」锁死测试**必须仍绿**。内容走**独立** `content_sinks` 通道，只进面板内存。
- **内容只在内存**：`call_buffer` 上界 + `payload_max_bytes`（默认 16384）单条上界；重启即丢；绝不写 audit/tracing/metrics。
- **始终捕获**：无独立开关——面板启用即捕获；面板关闭则 `content_sinks` 为空、`call_tool` 零额外开销。
- **截断 UTF-8 安全**；`CallContentSink::record` 非阻塞、不 panic（trait 契约）。
- **XSS**：CallDetail 用 `<pre>{...}</pre>` 文本插值；无 `{@html}`（`assets.rs` 守护测试强制）。
- **dist 同步**：改 `ui/src` 必 `npm run build` 并提交 `ui/dist`。

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/observe/src/lib.rs` | `CallContent` + `CallContentSink`（紧邻 `CallSink`） | 修改 |
| `crates/downstream/src/lib.rs` | `GatewayServer` 加 `content_sinks`/`payload_max_bytes`；`call_tool` 捕获+截断+扇出；`cap_json`/`cap_response` helper | 修改 |
| `crates/downstream/src/http.rs` | `build_router` 加两参数、透传 `GatewayServer::new` | 修改 |
| `crates/config/src/lib.rs` | `[dashboard].payload_max_bytes`（默认 16384、`validate` 拒绝 0） | 修改 |
| `crates/dashboard/src/calls.rs` | `CallRingSink` 改 `impl CallContentSink`；`StoredCall` 加 `content`；`CallItem` 加内容字段；`query` 省略/`get` 含内容 | 修改 |
| `crates/mcpgw/src/main.rs` | 把 `CallRingSink` 注入 `content_sinks`（非 `sinks`）；透传 `payload_max_bytes` 给 http+stdio 两个 `GatewayServer` | 修改 |
| `crates/dashboard/ui/src/lib/CallDetail.svelte` | 展示 Arguments / Result（截断/未保留提示） | 修改 |
| `crates/mcpgw/tests/dashboard.rs` | e2e：call_tool 后详情含 args/result；列表不含 | 修改 |
| `docs/L1-overview.md` / `docs/L2-components/*` / `docs/L3-details/*` / `docs/L4-api/*` | 分层文档同步 | 修改 |

---

## Task 1：`observe` —— `CallContent` + `CallContentSink`

**Files:**
- Modify: `crates/observe/src/lib.rs`（紧邻 `CallSink` trait 之后）
- Test: `crates/observe/src/lib.rs`（同文件 `#[cfg(test)]`）

新增内容载荷类型与扇出契约，与仅元数据的 `CallRecord` 物理隔离。

- [ ] **Step 1: 写失败测试**

在 `crates/observe/src/lib.rs` 的测试模块（文件末尾 `#[cfg(test)] mod tests` 若无则新建）追加：
```rust
#[cfg(test)]
mod content_tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn call_content_sink_receives_meta_and_content() {
        struct Cap(Mutex<Vec<(String, String)>>); // (meta_tool, args)
        impl CallContentSink for Cap {
            fn record(&self, meta: &CallRecord, content: &CallContent) {
                self.0.lock().unwrap().push((meta.meta_tool.as_str().to_string(), content.args.clone()));
            }
        }
        let cap = Cap(Mutex::new(Vec::new()));
        let meta = CallRecord {
            ts_unix_ms: 0,
            meta_tool: MetaTool::CallTool,
            target_tool: Some("s__t".into()),
            upstream: Some("s".into()),
            latency_ms: 1,
            outcome: CallOutcome::Ok,
            error_kind: None,
            arg_bytes: 0,
            result_bytes: 0,
        };
        let content = CallContent {
            args: "{\"x\":1}".into(),
            args_truncated: false,
            result: "ok".into(),
            result_truncated: false,
        };
        cap.record(&meta, &content);
        let got = cap.0.lock().unwrap();
        assert_eq!(got[0], ("call_tool".to_string(), "{\"x\":1}".to_string()));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p observe content_tests`
Expected: 编译错误 `cannot find type CallContent`/`CallContentSink`。

- [ ] **Step 3: 实现（lib.rs，紧接 `pub trait CallSink` 之后）**

```rust
/// One call's content payload (args + result), captured ONLY into the dashboard's in-memory ring —
/// physically separate from the metadata-only `CallRecord`, so argument/result content never reaches
/// the tracing/audit sinks. Fields are already-serialized, already-truncated JSON text (easy to
/// store / substring-search / render in `<pre>`); `*_truncated` flags whether the cap was hit.
#[derive(Debug, Clone)]
pub struct CallContent {
    pub args: String,
    pub args_truncated: bool,
    pub result: String,
    pub result_truncated: bool,
}

/// Fan-out target for call CONTENT. Gets both the metadata `CallRecord` and the `CallContent`, so
/// the dashboard ring can store a rich record without duplicating the metadata fields. Like
/// `CallSink`/`DiscoverySink`, implementations MUST be non-blocking and MUST NOT panic.
pub trait CallContentSink: Send + Sync {
    fn record(&self, meta: &CallRecord, content: &CallContent);
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p observe content_tests`
Expected: PASS。

- [ ] **Step 5: 全 crate 验证 + 提交**

Run: `cargo test -p observe && cargo clippy -p observe --all-targets -- -D warnings && cargo fmt -p observe --check`
Expected: 全过、无 warning、无 diff。
```bash
git add crates/observe/src/lib.rs
git commit -m "feat(observe): add CallContent + CallContentSink (content fan-out contract)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：`config` —— `[dashboard].payload_max_bytes`

**Files:**
- Modify: `crates/config/src/lib.rs`（`DashboardConfig` 结构体 + `Default` + `validate`，镜像现有 `call_buffer`）
- Test: `crates/config/src/lib.rs`（同文件 `#[cfg(test)]`）

- [ ] **Step 1: 写失败测试**

在 `crates/config/src/lib.rs` 测试模块追加：
```rust
    #[test]
    fn dashboard_payload_max_bytes_defaults_to_16384() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(cfg.dashboard.payload_max_bytes, 16384);
    }

    #[test]
    fn dashboard_payload_max_bytes_zero_is_rejected() {
        let cfg = Config::from_toml_str("[dashboard]\nenabled = true\npayload_max_bytes = 0\n").unwrap();
        let err = cfg.validate().expect_err("payload_max_bytes=0 must be rejected");
        assert!(err.to_string().contains("payload_max_bytes"), "got: {err}");
    }
```
> 注：`from_toml_str` 内部已调 `validate()`，故 `=0` 用例直接 `from_toml_str(...).expect_err(...)`（与既有 `call_buffer` 测试同模式）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p config dashboard_payload_max_bytes`
Expected: 编译错误 `no field payload_max_bytes`。

- [ ] **Step 3: 实现**

`DashboardConfig` 结构体里、`call_buffer` 字段之后加：
```rust
    /// Per-call payload (args/result) capture cap in bytes for the Calls detail view. Must be > 0.
    pub payload_max_bytes: usize,
```
`impl Default for DashboardConfig` 里、`call_buffer: 2000,` 之后加：
```rust
            payload_max_bytes: 16384,
```
`validate()` 里、`if self.dashboard.enabled { ... }` 块内 `call_buffer == 0` 校验之后加：
```rust
            if self.dashboard.payload_max_bytes == 0 {
                return Err(ConfigError::Invalid(
                    "[dashboard].payload_max_bytes must be > 0".into(),
                ));
            }
```

- [ ] **Step 4: 跑测试确认通过 + 全验证 + 提交**

Run: `cargo test -p config dashboard_payload_max_bytes && cargo test -p config && cargo fmt -p config --check`
Expected: 全过、无 diff。
```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): add [dashboard].payload_max_bytes (default 16384, must be > 0)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：后端集成 —— 埋点捕获 + `CallRingSink` 改内容通道 + main.rs 接线

**Files:**
- Modify: `crates/downstream/src/lib.rs`（`GatewayServer` 加字段/参数、`call_tool` 捕获、`cap_*` helper），`crates/downstream/src/http.rs`（`build_router` 加两参）
- Modify: `crates/dashboard/src/calls.rs`（`CallRingSink` 改 `impl CallContentSink`、`StoredCall`/`CallItem` 加内容、`to_item(with_content)`）
- Modify: `crates/mcpgw/src/main.rs`（`CallRingSink` 进 `content_sinks`、透传 `payload_max_bytes`）

> 这三处必须同一提交（sink trait 切换牵动 main.rs 的 cast）。先做 dashboard + downstream 的可单测部分（TDD），再接 main.rs 让整体编译；边界扇出由 Task 5 的 e2e 覆盖。

### 3A. dashboard `calls.rs`

- [ ] **Step 1: 写失败测试**

在 `crates/dashboard/src/calls.rs` 测试模块：把现有 helper `rec(...)` 旁补一个 `content()` helper，并新增内容测试；同时**现有用 `ring.record(&rec(...))` 的测试要改成 `ring.record(&rec(...), &content())`**（CallContentSink 双参）。先加新测试（会编译失败）：
```rust
    fn content() -> observe::CallContent {
        observe::CallContent {
            args: "{\"text\":\"hi\"}".into(),
            args_truncated: false,
            result: "{\"ok\":true}".into(),
            result_truncated: false,
        }
    }

    #[test]
    fn ring_stores_content_detail_includes_list_omits() {
        let ring = CallRingSink::new(10);
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__a"), CallOutcome::Ok, 1), &content());
        // list (query) omits content:
        let (items, _) = ring.query(&CallFilter::default(), 10, 0);
        assert!(items[0].args.is_none(), "list omits args");
        assert!(items[0].result.is_none(), "list omits result");
        // detail (get) includes content:
        let d = ring.get(0).expect("seq 0");
        assert_eq!(d.args.as_deref(), Some("{\"text\":\"hi\"}"));
        assert_eq!(d.result.as_deref(), Some("{\"ok\":true}"));
        assert!(!d.args_truncated);
    }
```

- [ ] **Step 2: 跑确认失败** — `cargo test -p dashboard calls::` → 编译错误（`record` 签名变了 / `CallItem` 无 `args`）。

- [ ] **Step 3: 实现（calls.rs）**

1. import：把 `use observe::{CallRecord, CallSink};` 改为 `use observe::{CallContent, CallContentSink, CallRecord};`（`CallSink` 不再用）。
2. `CallItem` 结构体末尾（`result_bytes` 之后）加：
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub args_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub result_truncated: bool,
```
并在文件中加私有 helper（`CallItem` 定义附近）：
```rust
fn is_false(b: &bool) -> bool {
    !*b
}
```
3. `StoredCall` 加字段：
```rust
struct StoredCall {
    seq: u64,
    record: CallRecord,
    content: CallContent,
}
```
4. `to_item` 改为带 `with_content` 参数：
```rust
    fn to_item(&self, with_content: bool) -> CallItem {
        let r = &self.record;
        let (args, args_truncated, result, result_truncated) = if with_content {
            let c = &self.content;
            (Some(c.args.clone()), c.args_truncated, Some(c.result.clone()), c.result_truncated)
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
```
5. `query` 投影不含内容：把 `.map(|s| s.to_item())` 改为 `.map(|s| s.to_item(false))`。
6. `get` 投影含内容：把 `.map(|s| s.to_item())` 改为 `.map(|s| s.to_item(true))`。
7. 把 `impl CallSink for CallRingSink { fn record(&self, rec: &CallRecord) {...} }` 整段替换为：
```rust
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
```
8. 现有测试里所有 `ring.record(&rec(...))` 调用改为 `ring.record(&rec(...), &content())`（query/filter/pagination 等测试只断言元数据，逻辑不变）。

- [ ] **Step 4: 跑确认通过** — `cargo test -p dashboard calls::`（含新测试 + 改过的旧测试全过）。

### 3B. downstream `lib.rs` + `http.rs`

- [ ] **Step 5: 写失败测试（cap helper，纯函数）**

在 `crates/downstream/src/lib.rs` 测试模块追加：
```rust
    #[test]
    fn truncate_utf8_respects_char_boundary() {
        let (s, t) = truncate_utf8("héllo".to_string(), 2); // 'h'=1B, 'é'=2B -> cap 2 cuts mid-char
        assert_eq!(s, "h"); // backs off to char boundary
        assert!(t);
        let (s2, t2) = truncate_utf8("hi".to_string(), 8);
        assert_eq!(s2, "hi");
        assert!(!t2);
    }

    #[test]
    fn cap_json_serializes_and_truncates() {
        let (s, t) = cap_json(&serde_json::json!({"a": 1}), 100);
        assert_eq!(s, "{\"a\":1}");
        assert!(!t);
        let (_s, t2) = cap_json(&serde_json::json!({"a": "xxxxxxxxxx"}), 5);
        assert!(t2, "long value truncated");
    }
```

- [ ] **Step 6: 跑确认失败** — `cargo test -p downstream truncate_utf8` → `cannot find function`.

- [ ] **Step 7: 实现（downstream lib.rs）**

1. `GatewayServer` 结构体加字段（`discovery` 之后）：
```rust
    content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>,
    payload_max_bytes: usize,
```
2. `GatewayServer::new` 加两参数并构造：签名末尾加 `content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>, payload_max_bytes: usize,`，`Self { ... }` 末尾加 `content_sinks, payload_max_bytes,`。
3. 加 module 级 helper（`json_len` 附近）：
```rust
/// UTF-8-safe truncation to at most `cap` bytes. Returns (possibly-truncated string, truncated?).
fn truncate_utf8(s: String, cap: usize) -> (String, bool) {
    if s.len() <= cap {
        return (s, false);
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Compact-serialize `value` then UTF-8-truncate to `cap`. Serialization failure -> ("<unserializable>", false).
fn cap_json<T: serde::Serialize>(value: &T, cap: usize) -> (String, bool) {
    match serde_json::to_string(value) {
        Ok(s) => truncate_utf8(s, cap),
        Err(_) => ("<unserializable>".to_string(), false),
    }
}

/// Serialize the call response (success result, or the error string) then truncate.
fn cap_response(response: &Result<CallToolResult, McpError>, cap: usize) -> (String, bool) {
    match response {
        Ok(r) => cap_json(r, cap),
        Err(e) => truncate_utf8(e.to_string(), cap),
    }
}
```
4. 在 `call_tool` 里、`for sink in self.sinks.iter() { sink.record(&rec); }` 之后、`response` 之前，插入内容扇出：
```rust
        if !self.content_sinks.is_empty() {
            let (args_s, args_truncated) = cap_json(&args, self.payload_max_bytes);
            let (result_s, result_truncated) = cap_response(&response, self.payload_max_bytes);
            let content = observe::CallContent {
                args: args_s,
                args_truncated,
                result: result_s,
                result_truncated,
            };
            for s in self.content_sinks.iter() {
                s.record(&rec, &content);
            }
        }
```

- [ ] **Step 8: 实现（http.rs build_router 透传）**

`crates/downstream/src/http.rs` 的 `build_router` 签名末尾加：
```rust
    content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>,
    payload_max_bytes: usize,
```
`GatewayServer::new(...)` 调用末尾加 `content_sinks.clone(), payload_max_bytes,`（`content_sinks` 被 `move` 闭包捕获，需 `.clone()`）。

### 3C. main.rs 接线

- [ ] **Step 9: 实现（main.rs）**

1. 把 `dashboard_calls` 块改成**不**进 `sink_vec`（它现在是 `CallContentSink`，不是 `CallSink`）：
```rust
    // Per-call ring for the dashboard Calls drill-down (only when dashboard enabled). Fed via the
    // CONTENT channel (CallContentSink) so it carries args/result; bounded by [dashboard].call_buffer.
    let dashboard_calls = if cfg.dashboard.enabled {
        Some(Arc::new(dashboard::CallRingSink::new(cfg.dashboard.call_buffer)))
    } else {
        None
    };
    let content_sinks: Arc<[Arc<dyn observe::CallContentSink>]> = match &dashboard_calls {
        Some(c) => Arc::from(vec![c.clone() as Arc<dyn observe::CallContentSink>]),
        None => Arc::from(Vec::new()),
    };
    let payload_max_bytes = cfg.dashboard.payload_max_bytes;
```
2. http `build_router(...)` 调用末尾加 `content_sinks.clone(), payload_max_bytes,`。
3. stdio `GatewayServer::new(...)` 调用末尾加 `content_sinks.clone(), payload_max_bytes,`。
4. `AppState { ... calls: dashboard_calls.clone(), ... }` 不变（仍是 `Option<Arc<CallRingSink>>`）。

- [ ] **Step 10: 全后端验证 + 提交**

Run: `cargo test -p observe -p downstream -p dashboard -p mcpgw && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all --check`
Expected: 全过、无 warning、无 diff。特别确认：downstream 既有的「`CallRecord` 仅元数据」相关测试仍绿（元数据路径未变）。
```bash
git add crates/downstream/src/lib.rs crates/downstream/src/http.rs crates/dashboard/src/calls.rs crates/mcpgw/src/main.rs
git commit -m "feat(dashboard): capture call args/result into CallRingSink via CallContentSink

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：前端 —— CallDetail 展示 Arguments / Result

**Files:**
- Modify: `crates/dashboard/ui/src/lib/CallDetail.svelte`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

在 Call detail 的元数据表之后，新增 Arguments 与 Result 两块（失败时 Result 即含上游错误文本）。`<pre>` 文本插值、无 `{@html}`。

- [ ] **Step 1: 实现**

在 `<script>` 里、`when` 函数之后加 `pretty` helper：
```js
  function pretty(s) {
    try { return JSON.stringify(JSON.parse(s), null, 2); } catch (_) { return s; }
  }
```
在元数据 `</table>`（第 40 行）之后、`{:else if !error}`（第 41 行）之前，插入：
```svelte
  <h3>Arguments{#if item.args_truncated} <span class="muted">(truncated)</span>{/if}</h3>
  {#if item.args != null}
    <pre class="schema">{pretty(item.args)}</pre>
  {:else}
    <p class="muted">(content not retained)</p>
  {/if}

  <h3>Result{#if item.result_truncated} <span class="muted">(truncated)</span>{/if}</h3>
  {#if item.result != null}
    <pre class="schema">{pretty(item.result)}</pre>
  {:else}
    <p class="muted">(content not retained)</p>
  {/if}
```
> 复用 M3 已有的 `.schema`（等宽 `<pre>` 框）与 `h3` 样式。失败调用的上游错误文本就在 Result 里（`outcome != ok` 时）；`error_kind` 分类已在上方表里。

- [ ] **Step 2: 构建 + 内嵌断言**

Run: `cd crates/dashboard/ui && npm run build && cd ../../.. && cargo test -p dashboard assets::`
Expected: 构建成功（a11y 警告非致命）；`assets::` 3/3（含 `{@html}` 守护——只用 `<pre>{...}</pre>` 文本插值）。

- [ ] **Step 3: 提交**

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): show Arguments/Result in CallDetail

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5：e2e 断言 + 分层文档 + 四道门禁

**Files:**
- Modify: `crates/mcpgw/tests/dashboard.rs`（mock-上游 e2e 加内容捕获断言）
- Modify: `docs/L4-api/{observe-lib,downstream-lib,dashboard}.md`、`docs/L3-details/dashboard.md`、`docs/L2-components/{observe,downstream,dashboard}.md`、`docs/L1-overview.md`

### e2e

- [ ] **Step 1: 扩 mock-上游 e2e**

在 `crates/mcpgw/tests/dashboard.rs` 的 `dashboard_detail_endpoints_with_mock_upstream`（它已驱动 `call_tool mock__echo {text:"hi"}`）里、`client.cancel()` 之前追加：
```rust
    // M1 payload capture: the call_tool above captured args + result into the live ring.
    let calls: serde_json::Value = http
        .get(format!("{base}/api/calls?source=live&meta=call_tool"))
        .send().await.unwrap().json().await.unwrap();
    // list omits content:
    assert!(calls["items"][0].get("args").is_none(), "list omits args");
    let cid = calls["items"][0]["id"].as_str().expect("a call_tool id").to_string();
    // detail includes content:
    let cd: serde_json::Value = http
        .get(format!("{base}/api/calls/{cid}"))
        .send().await.unwrap().json().await.unwrap();
    let args = cd["args"].as_str().expect("detail has args");
    assert!(args.contains("hi"), "args contain the echoed text: {args}");
    assert!(cd["result"].as_str().is_some(), "detail has result");
```
Run（需先建 mock-stdio）：`cargo build -p upstream --features testkit --bin mock-stdio && MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored`
Expected: 2 passed（mock 测试实跑、不跳过）。

### 文档（READ 后改，忠实于代码）

- [ ] **Step 2: L4** ——
  - `docs/L4-api/observe-lib.md`：新增 `CallContent { args, args_truncated, result, result_truncated }` 与 `trait CallContentSink { fn record(&self, meta: &CallRecord, content: &CallContent) }`（仅内存内容通道，与仅元数据 `CallRecord` 隔离）。
  - `docs/L4-api/downstream-lib.md`：`GatewayServer` 加 `content_sinks`/`payload_max_bytes`；`call_tool` 在元数据扇出后构造截断 `CallContent` 扇出到 `content_sinks`；`cap_json`/`cap_response`/`truncate_utf8`（UTF-8 安全截断）；`build_router` 加两参。
  - `docs/L4-api/dashboard.md`：`CallRingSink` 改实现 `CallContentSink`（存元数据+内容）；`CallItem` 增 `args?`/`args_truncated`/`result?`/`result_truncated`（**列表省略、详情返回**）；`/api/calls/{id}` 返回内容、`/api/calls` 不含；`StoredCall` 加 `content`、`to_item(with_content)`。`config` 段记 `[dashboard].payload_max_bytes`（默认 16384）。
- [ ] **Step 3: L3（`docs/L3-details/dashboard.md`）** —— 新增「调用内容捕获」段：内容走独立 `CallContentSink` 通道（元数据 `CallRecord`→tracing/audit/metrics **不变**、审计仍洁净）；内容**只在内存**（`call_buffer`×`payload_max_bytes` 上界、重启即丢）；单条 UTF-8 截断；详情含内容、列表轻。测试覆盖补 cap helper / 环内容 / config。
- [ ] **Step 4: L2** —— `docs/L2-components/observe.md` 加 `CallContent`/`CallContentSink`；`downstream.md` 加 content_sinks/payload_max_bytes 与捕获职责；`dashboard.md` 的 `CallRingSink` 改记「实现 `CallContentSink`、存元数据+内容」、依赖里 sink 通道说明。
- [ ] **Step 5: L1（`docs/L1-overview.md`）** —— 路线图加：`子系统 A · 调用内容捕获 ✅ —— 每次调用把 args/result（含上游错误文本）经独立 CallContentSink 通道捕获进面板内存环（call_buffer×payload_max_bytes 上界、重启即丢、单条 UTF-8 截断）；CallDetail 展示、列表不含；元数据 CallRecord→tracing/audit/metrics 不变（审计仍仅元数据）`。测试计数行用 Step 6 实测回填。

### 门禁

- [ ] **Step 6: 四道门禁**

```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿。记录 `N passed / M ignored` 回填 L1；并 `cargo build -p upstream --features testkit --bin mock-stdio && MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored`（2 passed）；并 `cd crates/dashboard/ui && npm run build && cd ../../.. && git status --short crates/dashboard/ui/dist`（应为空）。

- [ ] **Step 7: 提交**

```bash
git add crates/mcpgw/tests/dashboard.rs docs/
git commit -m "test+docs: M1 payload capture e2e + sync L1-L4 (CallContent capture)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## M1 完成判据（DoD）

- [ ] `observe::CallContent` + `CallContentSink` 落地；`CallRecord` 仍仅元数据（锁死测试不变、绿）。
- [ ] `downstream::call_tool` 在元数据扇出外，按 `payload_max_bytes` 截断后把 args/result 扇出到 `content_sinks`（面板启用时）；helper UTF-8 安全。
- [ ] `CallRingSink` 实现 `CallContentSink`、存元数据+内容；`CallItem` 详情含 args/result、列表省略；`/api/calls/{id}` 返回内容。
- [ ] `[dashboard].payload_max_bytes`（默认 16384、`=0` 拒绝）；main.rs 把环注入 `content_sinks`、透传 cap 给 http+stdio。
- [ ] CallDetail 展示 Arguments/Result（截断/未保留提示）；无 `{@html}`。
- [ ] 内容只在内存、绝不进 audit/tracing/metrics。
- [ ] 四道门禁绿；mock e2e（详情含 args/result、列表不含）实跑通过；dist 同步；L1-L4 文档一致。

## 给实现者的备注
- **DRY**：内容只走 `CallContentSink` 一条通道；元数据 `CallRecord` 路径**一字不改**。
- **YAGNI**：M1 只做捕获 + 详情展示，**不**做过滤（M2）。
- **隐私**：内容仅内存、重启即丢、单条截断；注释/文档点明「绝不进 audit/tracing」。
- **锁纪律**：环 `Mutex` 锁内分配 seq、不跨 `.await`、`.lock().unwrap_or_else(|e| e.into_inner())`。
- **每改 ui/src 必 `npm run build` 并提交 dist**。
