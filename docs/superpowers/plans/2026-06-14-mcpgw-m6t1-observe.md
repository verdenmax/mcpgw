# M6.T1：结构化调用日志 + 追踪（observe crate + downstream 埋点）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给网关的每次元工具调用产出**结构化、仅元数据**的观测记录（工具名/上游/延迟/结果/错误类别/参数与返回的大小），经一个可插拔 sink 架构落地——T1 先做 `TracingSink`（结构化 `tracing` 日志）。**绝不记载参数/返回内容**。

**Architecture:** 新 `observe` crate 定义 `CallRecord`（纯元数据）+ `CallSink` trait + `TracingSink`（+ testkit `CaptureSink`）。`downstream::GatewayServer` 持有 `Arc<[Arc<dyn CallSink>]>`，在元工具处理边界计时、分类、构造 `CallRecord` 并 fan-out。`metatools` 保持纯逻辑。T3（JSONL 审计）将再加一个 sink，无需改埋点。

**Tech Stack:** Rust 2021 / serde / serde_json / tracing / 复用现有 rmcp downstream server。

**Spec:** `docs/superpowers/specs/2026-06-14-mcpgw-m6-observability-design.md`（本计划只实现其中的 **T1**；T3 走自己的 spec/计划）。

---

## 已确认的关键事实（实现时照用）

- 当前 `downstream::GatewayServer`：`#[derive(Clone)] struct { state: Arc<GatewayState>, default_top_k: usize }`；`new(state, default_top_k)`。`impl ServerHandler::call_tool` 在 `crates/downstream/src/lib.rs:98-150`，按 `request.name` 分三臂（search_tools/get_tool_details/call_tool）+ `other => McpError::invalid_params`。
- `metatools::call_tool` 返回 `Result<CallToolResult, MetaError>`；downstream 现在 `Err(e) => Ok(CallToolResult::error(...))`。`metatools::MetaError`（公开）变体：`ToolNotFound(String)` / `UpstreamUnavailable(String)` / `Timeout` / `Call(String)`。
- `GatewayServer::new` 调用点：`crates/downstream/src/http.rs:60`（在 `build_router` 的 service 工厂闭包内）、`crates/mcpgw/src/main.rs:268`（stdio）、`crates/downstream/tests/common/mod.rs:22`。`build_router` 调用点：`crates/mcpgw/src/main.rs:256`、`crates/downstream/tests/http_server.rs:30`。
- mcpgw 已初始化 `tracing_subscriber::fmt()`（EnvFilter 默认 info，`main.rs:226`）——`TracingSink` 直接复用。
- testkit 约定：mock/capture 类放在该 crate 的 `testkit` feature 后（见 `retrieval::MockEmbedder`、`MockChatModel`），消费方 dev-dep 该 crate 带 `features=["testkit"]`。
- **本仓库强制 `cargo fmt --all --check`**；每个 task 提交前先 `cargo fmt -p <crate>` 并确认 `--check` 干净。
- 分层文档（L1–L4 + README + roadmap）是 DoD。

## File Structure

| 文件 | 职责 | 任务 |
|------|------|------|
| `crates/observe/Cargo.toml` + `src/lib.rs`（新 crate）| `CallRecord`/`MetaTool`/`CallOutcome`/`CallSink`/`TracingSink` + testkit `CaptureSink` + 单测 | T1.1 |
| `Cargo.toml`(workspace) | members 加 `crates/observe` | T1.1 |
| `crates/downstream/Cargo.toml` | dep `observe`；dev-dep `observe`(testkit) | T1.2 |
| `crates/downstream/src/lib.rs` | `GatewayServer` 持 sinks；`call_tool` 计时+分类+构造 `CallRecord`+fan-out；`classify(MetaError)` 辅助 | T1.2 |
| `crates/downstream/src/http.rs` | `build_router` 增 sinks 参数 | T1.2 |
| `crates/downstream/tests/common/mod.rs` | `connect_to_gateway` 默认空 sinks + 新 `_with_sinks` 变体 | T1.2 |
| `crates/downstream/tests/{server,http_server}.rs` | 更新调用点 + 新增 CaptureSink 埋点断言测试 | T1.2 |
| `crates/mcpgw/Cargo.toml` + `src/main.rs` | dep `observe`；装配默认 `[TracingSink]` 注入 build_router + GatewayServer::new | T1.2 |
| `docs/L1`–`L4` / `README` / roadmap | 分层文档 | T1.3 |

## 前置：建分支

```bash
git switch -c feat/m6t1-observe
```

---

### Task 1: `observe` crate（`CallRecord` + `CallSink` + `TracingSink` + testkit `CaptureSink`）

**Files:**
- Create: `crates/observe/Cargo.toml`、`crates/observe/src/lib.rs`
- Modify: `Cargo.toml`(workspace) members

- [ ] **Step 1: workspace 注册**

根 `Cargo.toml` 的 `members` 数组末尾加 `"crates/observe"`。

- [ ] **Step 2: `crates/observe/Cargo.toml`**

```toml
[package]
name = "observe"
version = "0.1.0"
edition = { workspace = true }

[features]
testkit = []

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros"] }
```

- [ ] **Step 3: `crates/observe/src/lib.rs`**

```rust
//! `observe`: structured, metadata-only observation of gateway meta-tool calls.
//!
//! Defines `CallRecord` (NO argument/result payloads — only sizes), the `CallSink` trait, and a
//! `TracingSink`. This is the storage-free, HTTP-free seam that T1 (tracing) and T3 (audit JSONL)
//! share: one record is built at the call boundary and fanned out to every configured sink.

use serde::Serialize;

/// Which meta-tool was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaTool {
    SearchTools,
    GetToolDetails,
    CallTool,
}

/// The outcome of a meta-tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Ok,
    Error,
    Timeout,
}

/// Metadata-only record of one meta-tool call. By construction it carries NO argument or result
/// content — only sizes (`arg_bytes`/`result_bytes`) — so it can never leak secrets/PII into logs
/// or the audit trail.
#[derive(Debug, Clone, Serialize)]
pub struct CallRecord {
    pub ts_unix_ms: u64,
    pub meta_tool: MetaTool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub latency_ms: u64,
    pub outcome: CallOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
    pub arg_bytes: usize,
    pub result_bytes: usize,
}

impl CallRecord {
    /// Current unix time in milliseconds (for `ts_unix_ms`).
    pub fn now_unix_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A sink for call observations. Implementations MUST be non-blocking and MUST NOT panic — an
/// observation failure must never affect the tool call itself.
pub trait CallSink: Send + Sync {
    fn record(&self, rec: &CallRecord);
}

/// T1 sink: emit each record as a structured `tracing` event (reusing the process subscriber).
pub struct TracingSink;

impl CallSink for TracingSink {
    fn record(&self, r: &CallRecord) {
        tracing::info!(
            meta_tool = ?r.meta_tool,
            target_tool = r.target_tool.as_deref(),
            upstream = r.upstream.as_deref(),
            latency_ms = r.latency_ms,
            outcome = ?r.outcome,
            error_kind = r.error_kind,
            arg_bytes = r.arg_bytes,
            result_bytes = r.result_bytes,
            "tool_call"
        );
    }
}

#[cfg(feature = "testkit")]
mod capture {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Test sink that captures every record for assertions.
    #[derive(Clone, Default)]
    pub struct CaptureSink {
        records: Arc<Mutex<Vec<CallRecord>>>,
    }
    impl CaptureSink {
        pub fn new() -> Self {
            Self::default()
        }
        /// Snapshot of all records seen so far.
        pub fn records(&self) -> Vec<CallRecord> {
            self.records.lock().unwrap().clone()
        }
    }
    impl CallSink for CaptureSink {
        fn record(&self, rec: &CallRecord) {
            self.records.lock().unwrap().push(rec.clone());
        }
    }
}
#[cfg(feature = "testkit")]
pub use capture::CaptureSink;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CallRecord {
        CallRecord {
            ts_unix_ms: 1,
            meta_tool: MetaTool::CallTool,
            target_tool: Some("github__create_issue".into()),
            upstream: Some("github".into()),
            latency_ms: 7,
            outcome: CallOutcome::Ok,
            error_kind: None,
            arg_bytes: 42,
            result_bytes: 99,
        }
    }

    #[test]
    fn serializes_enums_as_snake_case_short_strings() {
        let v: serde_json::Value = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["meta_tool"], "call_tool");
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["target_tool"], "github__create_issue");
        assert_eq!(v["arg_bytes"], 42);
    }

    #[test]
    fn record_is_metadata_only_no_payload_keys() {
        // The TYPE cannot carry argument/result content; lock that the serialized key set is a
        // subset of the allowed metadata keys (no "arguments"/"args"/"result"/"content"/"text").
        let v = serde_json::to_value(sample()).unwrap();
        let allowed: std::collections::HashSet<&str> = [
            "ts_unix_ms",
            "meta_tool",
            "target_tool",
            "upstream",
            "latency_ms",
            "outcome",
            "error_kind",
            "arg_bytes",
            "result_bytes",
        ]
        .into_iter()
        .collect();
        for key in v.as_object().unwrap().keys() {
            assert!(allowed.contains(key.as_str()), "unexpected key leaked: {key}");
        }
    }

    #[test]
    fn skips_none_optionals() {
        let mut r = sample();
        r.target_tool = None;
        r.upstream = None;
        r.error_kind = None;
        let v = serde_json::to_value(r).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("target_tool"));
        assert!(!obj.contains_key("upstream"));
        assert!(!obj.contains_key("error_kind"));
    }
}
```

- [ ] **Step 4: testkit 单测（CaptureSink）**

创建 `crates/observe/tests/capture.rs`（testkit 门控；验证 `CaptureSink` 实现 `CallSink` 且捕获记录）：
```rust
#![cfg(feature = "testkit")]
use observe::{CallOutcome, CallRecord, CallSink, CaptureSink, MetaTool};

fn rec(o: CallOutcome) -> CallRecord {
    CallRecord {
        ts_unix_ms: 0,
        meta_tool: MetaTool::SearchTools,
        target_tool: None,
        upstream: None,
        latency_ms: 0,
        outcome: o,
        error_kind: None,
        arg_bytes: 0,
        result_bytes: 0,
    }
}

#[test]
fn capture_sink_records_in_order() {
    let s = CaptureSink::new();
    s.record(&rec(CallOutcome::Ok));
    s.record(&rec(CallOutcome::Error));
    let got = s.records();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].outcome, CallOutcome::Ok);
    assert_eq!(got[1].outcome, CallOutcome::Error);
}
```
并在 `crates/observe/Cargo.toml` 末尾加：
```toml
[[test]]
name = "capture"
required-features = ["testkit"]
```

- [ ] **Step 5: 运行 + fmt + clippy**

```bash
cargo test -p observe --all-features
cargo fmt -p observe && cargo fmt -p observe -- --check
cargo clippy -p observe --all-targets --all-features -- -D warnings
```
Expected: 单测（3 个 lib + capture 1）全 PASS；fmt 干净；clippy 无告警。

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml crates/observe/
git commit -m "feat(observe): CallRecord + CallSink + TracingSink (metadata-only) (M6.T1 T1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

**Discipline:** 只建 `observe` crate（纯元数据类型 + sink + tracing sink + capture）。不碰 downstream（T1.2）。

---

### Task 2: downstream 埋点 + 全调用点接线 + 测试

破坏性签名变更（`GatewayServer::new`、`build_router` 增 sinks）——一次性更新 downstream/mcpgw/测试，保持编译+测试绿。

**Files:**
- Modify: `crates/downstream/Cargo.toml`、`crates/downstream/src/lib.rs`、`crates/downstream/src/http.rs`
- Modify: `crates/downstream/tests/common/mod.rs`、`crates/downstream/tests/server.rs`、`crates/downstream/tests/http_server.rs`
- Modify: `crates/mcpgw/Cargo.toml`、`crates/mcpgw/src/main.rs`

- [ ] **Step 1: downstream 依赖 observe**

`crates/downstream/Cargo.toml`：`[dependencies]` 加 `observe = { path = "../observe" }`；`[dev-dependencies]` 加 `observe = { path = "../observe", features = ["testkit"] }`。

- [ ] **Step 2: `GatewayServer` 持 sinks + `classify` 辅助（`crates/downstream/src/lib.rs`）**

把 `GatewayServer` 结构与 `new` 改为：
```rust
/// The downstream MCP server. Holds shared gateway state, the default `top_k`, and the
/// observation sinks each meta-tool call is reported to.
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<GatewayState>,
    default_top_k: usize,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
}

impl GatewayServer {
    pub fn new(
        state: Arc<GatewayState>,
        default_top_k: usize,
        sinks: Arc<[Arc<dyn observe::CallSink>]>,
    ) -> Self {
        Self {
            state,
            default_top_k,
            sinks,
        }
    }
}

/// Classify a meta-tool call failure for the observation record.
fn classify(e: &metatools::MetaError) -> (observe::CallOutcome, Option<&'static str>) {
    use metatools::MetaError as E;
    use observe::CallOutcome as O;
    match e {
        E::Timeout => (O::Timeout, Some("timeout")),
        E::Call(_) => (O::Error, Some("upstream_call")),
        E::ToolNotFound(_) => (O::Error, Some("tool_not_found")),
        E::UpstreamUnavailable(_) => (O::Error, Some("upstream_unavailable")),
    }
}
```

- [ ] **Step 3: 重构 `call_tool`（埋点）**

把 `crates/downstream/src/lib.rs` 的整个 `async fn call_tool`（98-150）替换为：
```rust
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        use observe::{CallOutcome, CallRecord, MetaTool};

        let started = std::time::Instant::now();
        let args = request.arguments.unwrap_or_default();
        let arg_bytes = serde_json::to_string(&args).map(|s| s.len()).unwrap_or(0);

        // Each arm yields: (response, meta_tool, target_tool, outcome, error_kind).
        // The unknown-meta-name case returns a protocol error and is NOT recorded.
        let (response, meta_tool, target_tool, outcome, error_kind): (
            Result<CallToolResult, McpError>,
            MetaTool,
            Option<String>,
            CallOutcome,
            Option<&'static str>,
        ) = match request.name.as_ref() {
            "search_tools" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let top_k = args
                    .get("top_k")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(self.default_top_k);
                let snap = self.state.snapshot();
                let hits = metatools::search_tools(&snap, query, top_k).await;
                match serde_json::to_string(&hits) {
                    Ok(json) => (
                        Ok(CallToolResult::success(vec![Content::text(json)])),
                        MetaTool::SearchTools,
                        None,
                        CallOutcome::Ok,
                        None,
                    ),
                    Err(e) => (
                        Err(McpError::internal_error(e.to_string(), None)),
                        MetaTool::SearchTools,
                        None,
                        CallOutcome::Error,
                        Some("internal"),
                    ),
                }
            }
            "get_tool_details" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let snap = self.state.snapshot();
                match metatools::get_tool_details(&snap, name) {
                    Some(def) => match serde_json::to_string(def) {
                        Ok(json) => (
                            Ok(CallToolResult::success(vec![Content::text(json)])),
                            MetaTool::GetToolDetails,
                            None,
                            CallOutcome::Ok,
                            None,
                        ),
                        Err(e) => (
                            Err(McpError::internal_error(e.to_string(), None)),
                            MetaTool::GetToolDetails,
                            None,
                            CallOutcome::Error,
                            Some("internal"),
                        ),
                    },
                    None => (
                        Ok(CallToolResult::error(vec![Content::text(format!(
                            "no such tool: {name}"
                        ))])),
                        MetaTool::GetToolDetails,
                        None,
                        CallOutcome::Error,
                        Some("tool_not_found"),
                    ),
                }
            }
            "call_tool" => match args.get("name").and_then(|v| v.as_str()) {
                None => (
                    Ok(CallToolResult::error(vec![Content::text(
                        "missing required 'name'",
                    )])),
                    MetaTool::CallTool,
                    None,
                    CallOutcome::Error,
                    Some("invalid_params"),
                ),
                Some(name) => {
                    let inner = args.get("arguments").and_then(|v| v.as_object()).cloned();
                    let snap = self.state.snapshot();
                    match metatools::call_tool(&snap, self.state.registry(), name, inner).await {
                        Ok(result) => (
                            Ok(result),
                            MetaTool::CallTool,
                            Some(name.to_string()),
                            CallOutcome::Ok,
                            None,
                        ),
                        Err(e) => {
                            let (outcome, kind) = classify(&e);
                            (
                                Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                                MetaTool::CallTool,
                                Some(name.to_string()),
                                outcome,
                                kind,
                            )
                        }
                    }
                }
            },
            other => {
                // Unknown meta-tool name: protocol error, not a gateway tool call -> not recorded.
                return Err(McpError::invalid_params(
                    format!("unknown tool: {other}"),
                    None,
                ));
            }
        };

        let result_bytes = match &response {
            Ok(r) => serde_json::to_string(r).map(|s| s.len()).unwrap_or(0),
            Err(_) => 0,
        };
        let upstream = target_tool
            .as_deref()
            .and_then(|t| t.split_once("__").map(|(s, _)| s.to_string()));
        let rec = CallRecord {
            ts_unix_ms: CallRecord::now_unix_ms(),
            meta_tool,
            target_tool,
            upstream,
            latency_ms: started.elapsed().as_millis() as u64,
            outcome,
            error_kind,
            arg_bytes,
            result_bytes,
        };
        for sink in self.sinks.iter() {
            sink.record(&rec);
        }
        response
    }
```

- [ ] **Step 4: `build_router` 增 sinks 参数（`crates/downstream/src/http.rs`）**

`build_router` 签名加 `sinks: Arc<[Arc<dyn observe::CallSink>]>`（放在 `api_keys` 之后）；把闭包
`move || Ok(GatewayServer::new(state.clone(), default_top_k))` 改为
`move || Ok(GatewayServer::new(state.clone(), default_top_k, sinks.clone()))`。文件顶部按需 `use std::sync::Arc;`（若未引入）。

- [ ] **Step 5: 测试 harness（`crates/downstream/tests/common/mod.rs`）**

把 `connect_to_gateway` 保持现签名但内部用**空 sinks**，并新增一个带 sinks 的变体：
```rust
/// Build an empty sink list (no observation) for tests that don't assert on records.
pub fn no_sinks() -> std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> {
    Vec::new().into()
}

pub async fn connect_to_gateway(
    state: Arc<GatewayState>,
    default_top_k: usize,
) -> RunningService<RoleClient, ()> {
    connect_to_gateway_with_sinks(state, default_top_k, no_sinks()).await
}

/// Like `connect_to_gateway` but with explicit observation sinks (e.g. a `CaptureSink`).
pub async fn connect_to_gateway_with_sinks(
    state: Arc<GatewayState>,
    default_top_k: usize,
    sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]>,
) -> RunningService<RoleClient, ()> {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = GatewayServer::new(state, default_top_k, sinks);
    tokio::spawn(async move {
        let svc = server
            .serve(server_io)
            .await
            .expect("gateway server serves");
        let _ = svc.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}
```
（即把现有 `connect_to_gateway` 体搬进 `_with_sinks`，原函数转调它。文件顶部 `use` 需要时加 `observe`。）

- [ ] **Step 6: http_server 测试调用点（`crates/downstream/tests/http_server.rs`）**

把 `build_router(state, 8, "/mcp", api_keys)` 改为 `build_router(state, 8, "/mcp", api_keys, common_no_sinks())`——但 http_server.rs **不** `mod common`。故在 http_server.rs 内联一个空 sinks：
`let sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> = Vec::new().into();` 然后 `build_router(state, 8, "/mcp", api_keys, sinks)`。`spawn_http_gateway` 辅助也相应加 sinks 参数或内联空 sinks（择简）。http_server.rs 顶部加 `observe` 依赖可用（downstream dev-dep 已含）。

- [ ] **Step 7: 新增埋点断言测试（`crates/downstream/tests/server.rs`）**

新增（用 CaptureSink 注入，经现有 mock 上游 e2e harness 驱动三条路径）：
```rust
#[tokio::test]
async fn meta_tool_calls_are_observed_with_metadata() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    // search_tools -> meta_tool=SearchTools, no upstream/target.
    let _ = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query": "echo"}))),
        )
        .await
        .unwrap();

    // call_tool -> meta_tool=CallTool, target/upstream populated, outcome Ok.
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "mock__echo", "arguments": {"text": "hi"}
            }))),
        )
        .await
        .unwrap();

    // call a missing upstream tool -> outcome Error, error_kind tool_not_found.
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name": "mock__nope"}))),
        )
        .await
        .unwrap();

    client.cancel().await.unwrap();

    let recs = cap.records();
    assert_eq!(recs.len(), 3, "one record per meta-tool call");

    let search = &recs[0];
    assert_eq!(search.meta_tool, MetaTool::SearchTools);
    assert_eq!(search.outcome, CallOutcome::Ok);
    assert!(search.target_tool.is_none() && search.upstream.is_none());
    assert!(search.arg_bytes > 0);

    let call_ok = &recs[1];
    assert_eq!(call_ok.meta_tool, MetaTool::CallTool);
    assert_eq!(call_ok.outcome, CallOutcome::Ok);
    assert_eq!(call_ok.target_tool.as_deref(), Some("mock__echo"));
    assert_eq!(call_ok.upstream.as_deref(), Some("mock"));
    assert!(call_ok.error_kind.is_none());

    let call_err = &recs[2];
    assert_eq!(call_err.meta_tool, MetaTool::CallTool);
    assert_eq!(call_err.outcome, CallOutcome::Error);
    assert_eq!(call_err.error_kind, Some("tool_not_found"));
    assert_eq!(call_err.upstream.as_deref(), Some("mock"));
}
```
（`mock__nope` 是 mock 上游不存在的工具 → `metatools::call_tool` 返回 `MetaError::Call`/`ToolNotFound`？注意：mock 上游的 `tools/call` 对未知工具的行为决定 `error_kind`。**实现时先核对**：若 mock 对未知工具返回 rmcp 错误 → downstream 得 `MetaError::Call` → `error_kind="upstream_call"`；若上游 catalog 不含该名则 `metatools` 在 `snap.catalog.get` 处 `ToolNotFound`。`mock__nope` 不在 catalog（mock 只暴露 echo/greet/slow）→ `metatools::call_tool` 的 `snap.catalog.get(name)` 返回 None → `MetaError::ToolNotFound` → `error_kind="tool_not_found"`。以实际为准；若不符，改断言为实际类别并在报告里说明。)

- [ ] **Step 8: mcpgw 装配（`crates/mcpgw/Cargo.toml` + `src/main.rs`）**

`crates/mcpgw/Cargo.toml` `[dependencies]` 加 `observe = { path = "../observe" }`。
`crates/mcpgw/src/main.rs`：在装配 downstream 之前构造默认 sinks（仅 `TracingSink`）：
```rust
let sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> =
    vec![std::sync::Arc::new(observe::TracingSink) as std::sync::Arc<dyn observe::CallSink>].into();
```
然后把 `:256` 的 `build_router(state.clone(), cfg.retrieval.top_k, &h.path, api_keys)` 改为
`build_router(state.clone(), cfg.retrieval.top_k, &h.path, api_keys, sinks.clone())`；
把 `:268` 的 `GatewayServer::new(state_for_stdio, top_k)` 改为
`GatewayServer::new(state_for_stdio, top_k, sinks.clone())`。
（stdio 与 http 共享同一组 sinks。）

- [ ] **Step 9: 全测试 + fmt + clippy**

```bash
cargo test -p downstream -p mcpgw --all-features
cargo fmt -p downstream -p mcpgw && cargo fmt -p downstream -p mcpgw -- --check
cargo clippy -p downstream -p mcpgw --all-targets --all-features -- -D warnings
```
Expected: 全 PASS（含新埋点测试；既有 e2e 不回归）。

- [ ] **Step 10: 提交**

```bash
git add crates/downstream/ crates/mcpgw/Cargo.toml crates/mcpgw/src/main.rs Cargo.lock
git commit -m "feat(downstream,mcpgw): observe meta-tool calls via CallSink fan-out (M6.T1 T2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

**Discipline:** metatools 保持纯逻辑（不依赖 observe）。仅元数据：不把任何参数/返回内容放进 `CallRecord`。

---

### Task 3: 分层文档（L1–L4 + README + roadmap）

docs 必须忠实描述已落地代码——动手前先读对应源码与现有 doc 风格（如 `docs/L2-components/embedder.md`、`docs/L4-api/embedder-openai.md`）。

**Files:**
- Create: `docs/L4-api/observe-lib.md`、`docs/L2-components/observe.md`
- Modify: `docs/L4-api/downstream-lib.md`、`docs/L4-api/downstream-http.md`、`docs/L4-api/mcpgw-main.md`、`docs/L2-components/downstream.md`、`docs/L3-details/downstream.md`（如有）、`docs/L1-overview.md`、`docs/README.md`、`docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`

- [ ] **Step 1: 新建 L4 `docs/L4-api/observe-lib.md`**

覆盖 `crates/observe/src/lib.rs`：`MetaTool`/`CallOutcome` 枚举（snake_case 序列化）；`CallRecord`（**逐字段 + 强调仅元数据、`*_bytes` 不含内容、Option 字段 skip_if_none**）；`CallRecord::now_unix_ms`；`CallSink` trait（非阻塞、不 panic 契约）；`TracingSink`（结构化 `tracing::info!("tool_call", ...)`）；testkit `CaptureSink`。注明这是 T1/T3 共享的「埋点→多 sink」seam，T2 将再加 `MetricsSink`。

- [ ] **Step 2: 新建 L2 `docs/L2-components/observe.md`**

仿 `embedder.md`：`observe` crate 的职责（结构化、仅元数据的调用观测；定义 `CallRecord` + `CallSink` + `TracingSink`）；公开接口表；依赖（serde/serde_json/tracing）；被谁使用（`downstream` 埋点、`mcpgw` 装配 sinks）；不负责（存储=T3 的 JsonlSink、指标=T2）。链接 L4 observe-lib。

- [ ] **Step 3: 更新 L4 downstream 文档**

- `downstream-lib.md`：`GatewayServer` 现持 `sinks: Arc<[Arc<dyn observe::CallSink>]>`，`new(state, default_top_k, sinks)`；`call_tool` 现计时、分类 `error_kind`、构造 `CallRecord` 并向 sinks fan-out（仅元数据）；新增私有 `classify(MetaError)`。给出 `error_kind` 取值表（与 spec 一致）。
- `downstream-http.md`：`build_router` 现增 `sinks` 参数，注入每连接的 `GatewayServer`。

- [ ] **Step 4: 更新 L4 `mcpgw-main.md` + L2/L3 downstream**

- `mcpgw-main.md`：装配处新增「构造默认 `[TracingSink]` 并注入 stdio(`GatewayServer::new`) 与 http(`build_router`)」。
- `docs/L2-components/downstream.md`：职责补「每次元工具调用产出 `observe::CallRecord` 并 fan-out 到注入的 sinks」；依赖加 `observe`；导航加 observe。
- `docs/L3-details/downstream.md`（若存在）：补「调用观测」小节——埋点位置、计时口径、`error_kind` 分类表、`arg_bytes/result_bytes` 口径、仅元数据不变量、未知元工具名不记录。

- [ ] **Step 5: 更新 L1 `docs/L1-overview.md`**

新增 `observe` crate（结构化、仅元数据的调用观测；T1 起 `TracingSink`，T3 将加 JSONL 审计）；若有 crate 清单/架构图，补上。加一段「M6.T1 已完成」里程碑小结（仿 M2-A/M2.T5）。**默认仍 bm25** 等既有表述不变。测试计数块按实际更新（运行 `cargo test --all-features` 取数）。

- [ ] **Step 6: 更新 `docs/README.md` + roadmap**

- `README.md`：L2 清单加 `observe`；L4 清单加 `observe-lib.md`；里程碑覆盖说明加 **M6.T1（结构化调用日志/追踪）**。
- roadmap：`M6.T1` 标 `✅ 已完成（observe crate + TracingSink + downstream 埋点；仅元数据）`；可注「M6.T3 审计落库 待办；T2 指标、T4 code-mode 延后」。

- [ ] **Step 7: 校对 + 提交**

- 逐项核对新/改 doc 与真实代码一致（observe 类型、downstream 埋点、mcpgw 装配）；确认新增内链指向真实文件（`ls docs/L4-api/observe-lib.md docs/L2-components/observe.md`）。
- 产品文档（L1-L4 + README）无残留旧 `call_tool` 无埋点描述。

```bash
git add docs/
git commit -m "docs: L1-L4 + README + roadmap for M6.T1 call observation (M6.T1 T3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: 全量验证 + 合回 master

- [ ] **Step 1: 全量验证**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净；clippy 无告警；全测试 PASS（含 observe 单测/capture、downstream 埋点测试；`#[ignore]` 真实冒烟仍跳过）。

- [ ] **Step 2: 收尾**

1. 派发最终整体 code review（spec 覆盖、仅元数据不变量、sink 非阻塞/不 panic、downstream 埋点正确、全调用点无回归、文档同步）。
2. 处理 blocking 项（如有）。
3. 用 superpowers:finishing-a-development-branch 把 `feat/m6t1-observe` 合回 master（`--no-ff`，本地），删分支。

## 实现期需现场确认/可能回退的点（spec §6）
- `GatewayServer::new` / `build_router` 增 sinks：全调用点（http.rs、mcpgw、tests/common、tests/http_server）编译实证。
- `result_bytes` 复用：success 路径序列化 `CallToolResult` 取长度——确认对小结果开销可忽略；如成本敏感可改为累加 text content 长度（仍仅大小）。
- `mock__nope` 的 `error_kind`：取决于 mock 上游/`metatools` 对未知名的处理（预期 `tool_not_found`，因不在 catalog）；以实际为准，必要时改断言并说明。
- `Arc<[Arc<dyn CallSink>]>` 空列表构造需类型标注（`Vec::new().into()`）；`CaptureSink` 须 `Clone`（Arc 共享 records）以便保留断言句柄。
