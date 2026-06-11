# M1-B.2 实现计划：downstream server + `mcpgw serve` + list_changed

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把已建好的网关库接成一个活的 MCP server——任意 MCP 客户端经 stdio 连上 mcpgw 只看到 3 个元工具，网关在背后聚合多个真实 stdio 上游，并在上游 `list_changed` 时运行期刷新。

**Architecture:** 新增 `downstream` crate（`GatewayServer` 实现 rmcp `ServerHandler`，把 `tools/call` 派发到 `metatools` 的三个纯函数）；`upstream` 增加真实子进程 `connect_all` 与一个转发 `list_changed` 的 `UpstreamClientHandler`；`gateway::rebuild_snapshot` 改并发 ingest + per-ingest 超时（修死锁）并返回类型化遥测；`mcpgw serve` 装配全链路并跑一个 rebuild worker。

**Tech Stack:** Rust（workspace edition 2021，rustc ≥1.86）、rmcp 1.7（`default-features=false`，按 crate 启用 `server`/`client`/`transport-io`/`transport-child-process`）、tokio、serde_json、thiserror、tracing。检索沿用 `retrieval::Bm25Strategy`。

**Spec:** [`docs/superpowers/specs/2026-06-10-mcpgw-m1b2-downstream-serve.md`](../specs/2026-06-10-mcpgw-m1b2-downstream-serve.md)

---

## 已核实的 rmcp 1.7 API（写代码直接用，无需再 spike）

下列签名已对 `~/.cargo/registry/src/.../rmcp-1.7.0` 源码核实：

- `ServerHandler`（`rmcp::ServerHandler`，实现需 `Sized + Send + Sync + 'static`）：
  - `fn get_info(&self) -> ServerInfo`
  - `async fn list_tools(&self, Option<PaginatedRequestParams>, RequestContext<RoleServer>) -> Result<ListToolsResult, McpError>`
  - `async fn call_tool(&self, CallToolRequestParams, RequestContext<RoleServer>) -> Result<CallToolResult, McpError>`
- `ClientHandler`（`rmcp::ClientHandler`）：`async fn on_tool_list_changed(&self, NotificationContext<RoleClient>)`（默认 no-op，覆盖它即可）。
- 启动服务：`handler.serve(transport).await -> Result<RunningService<R, H>, E>`（`use rmcp::ServiceExt`）；`service.waiting().await` 运行至传输关闭。
- 传输：`rmcp::transport::stdio() -> (Stdin, Stdout)`；`rmcp::transport::TokioChildProcess::new(cmd)`；`use rmcp::transport::ConfigureCommandExt` 后 `tokio::process::Command::new(x).configure(|c| { c.args(..); c.env(k,v); })`。
- 构造：`Tool::new(name, desc, schema)`（`schema: impl Into<Arc<JsonObject>>`，`JsonObject = serde_json::Map<String,Value>`）；`ListToolsResult::with_all_items(Vec<Tool>)`；`CallToolResult::success(Vec<Content>)` / `CallToolResult::error(Vec<Content>)`；`Content::text(s)`。
- 错误：`use rmcp::ErrorData as McpError;`，构造 `McpError::invalid_params(msg, None)` / `McpError::internal_error(msg, None)`。
- server peer 发通知：`peer.notify_tool_list_changed().await`（`RoleServer` peer）。`#[tool]` 方法可加参数 `ctx: RequestContext<RoleServer>` 由宏注入，经 `ctx.peer` 拿到 peer。
- import 路径：`rmcp::{ServerHandler, ClientHandler, ServiceExt, RoleServer, RoleClient, ErrorData}`、`rmcp::service::{RequestContext, NotificationContext}`、`rmcp::model::{...}`、`rmcp::transport::{stdio, TokioChildProcess, ConfigureCommandExt}`。

## 已核实的内部 API

- `catalog::Catalog`：`new()`、`upsert(ToolDef)`、`get(&str)->Option<&ToolDef>`、`iter()`、`from_tooldefs(Vec)`、`len()`。`ToolDef{server,name,description,input_schema:Value}` 派生 `Serialize`，`qualified_name()->String`。
- `metatools`：`search_tools(&GatewaySnapshot,&str,usize)->Vec<ToolSummary>`、`get_tool_details(&GatewaySnapshot,&str)->Option<&ToolDef>`、`async call_tool(&GatewaySnapshot,&UpstreamRegistry,&str,Option<Map>)->Result<CallToolResult,MetaError>`；`MetaError`、`GatewaySnapshot::new(Catalog, Box<dyn RetrievalStrategy>)`、`ToolSummary{name,description}`（`Serialize`）。
- `gateway::GatewayState`：`new(&str)->Result<Self,String>`(本计划 Task 4 改为 `GatewayError`)、`registry()->&UpstreamRegistry`、`snapshot()->Arc<GatewaySnapshot>`、`async rebuild_snapshot()->Result<(),String>`(Task 4 改签名)。`Clone`。
- `upstream`：`UpstreamHandle::connect<T:AsyncRead+AsyncWrite+...>(server,transport)->Result<Self,UpstreamError>`、`with_call_timeout(Duration)`、`async ingest_into(&mut Catalog)->Result<usize,UpstreamError>`、`async call_tool(&str,Option<Map>)->Result<CallToolResult,UpstreamError>`、`server()->&str`、`async shutdown(self)`。`UpstreamRegistry`：`insert(Arc)`、`get(&str)->Option<Arc>`、`remove`、`server_names()->Vec<String>`。`UpstreamError::{Connect,Call,Timeout}`。testkit `MockUpstream`（echo/greet/slow）。
- `config::{Config, UpstreamConfig, UpstreamTransport::Stdio{command,args,env_passthrough}, RetrievalConfig{strategy,top_k}}`；`Config::from_toml_str(&str)`、`Config::default_from_empty()`。

---

## File Structure

新建 `downstream` crate（小而专：只做 rmcp server 适配，逻辑全在 metatools/gateway）：

- `crates/downstream/Cargo.toml`
- `crates/downstream/src/lib.rs` — `GatewayServer`（`ServerHandler`）+ `meta_tools()` 工具定义 + `call_tool` 派发。
- `crates/downstream/tests/common/mod.rs` — e2e 测试夹具（内存 duplex 起网关 + 测试 client + 接 mock 上游）。
- `crates/downstream/tests/server.rs` — list_tools / call_tool / list_changed 集成测试。

改动现有文件：

- `Cargo.toml`（workspace `members` 加 `crates/downstream`；如需 `futures` 则加 workspace dep——本计划用 `tokio::task::JoinSet`，无需 `futures`）。
- `crates/gateway/src/lib.rs` — 类型化 `GatewayError`、`RebuildSummary`、并发 ingest + per-ingest 超时。
- `crates/upstream/src/connection.rs` — `call_timeout()` getter；`UpstreamClientHandler`；`connect_with_trigger`。
- `crates/upstream/src/lib.rs` — 导出 `connect_all`/`ConnectSummary`/`UpstreamClientHandler`/`RebuildTrigger`（新增 `connect.rs` 模块）。
- `crates/upstream/src/connect.rs`（新）— `connect_all` / `connect_stdio_upstream` / `ConnectSummary`。
- `crates/upstream/src/testkit.rs` — 新增 `RevealingMockUpstream`（运行期可揭示新工具 + 发 list_changed）。
- `crates/config/src/lib.rs` — `ServerConfig` + `Config.server`。
- `crates/mcpgw/src/main.rs` — `serve` 子命令 + `run_serve` + `rebuild_worker`。
- `docs/`（L1-L4，最后一个 task 统一补/更新）。

---

## Task 1: 脚手架 `downstream` crate + `meta_tools()` + `GatewayServer` 骨架

**Files:**
- Create: `crates/downstream/Cargo.toml`
- Create: `crates/downstream/src/lib.rs`
- Modify: `Cargo.toml`（workspace members）

- [ ] **Step 1: 建 crate 清单 + 注册 workspace**

`crates/downstream/Cargo.toml`：

```toml
[package]
name = "downstream"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
gateway = { path = "../gateway" }
metatools = { path = "../metatools" }
catalog = { path = "../catalog" }
rmcp = { workspace = true, features = ["server", "transport-io"] }
serde_json = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
upstream = { path = "../upstream", features = ["testkit"] }
retrieval = { path = "../retrieval" }
rmcp = { workspace = true, features = ["client", "server", "transport-io"] }
tokio = { workspace = true, features = ["full"] }
serde_json = { workspace = true }
```

`Cargo.toml`（根）`members` 末尾加 `"crates/downstream"`：

```toml
members = ["crates/catalog", "crates/retrieval", "crates/config", "crates/mcpgw", "crates/upstream", "crates/metatools", "crates/gateway", "crates/downstream"]
```

- [ ] **Step 2: 写 `meta_tools()` 的失败测试**

`crates/downstream/src/lib.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_tools_are_exactly_the_three_with_schemas() {
        let tools = meta_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, ["search_tools", "get_tool_details", "call_tool"]);

        // search_tools 必含 query（必填）+ top_k（选填）。
        let search = &tools[0];
        let props = search.input_schema.get("properties").unwrap();
        assert!(props.get("query").is_some());
        assert!(props.get("top_k").is_some());
        let required = search.input_schema.get("required").unwrap();
        assert_eq!(required, &serde_json::json!(["query"]));
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p downstream meta_tools_are_exactly_the_three -- --nocapture`
Expected: 编译失败（`meta_tools` 未定义）。

- [ ] **Step 4: 实现 `meta_tools()` + `GatewayServer` 骨架**

`crates/downstream/src/lib.rs` 顶部（在 `#[cfg(test)]` 之前）：

```rust
//! mcpgw `downstream`: a rmcp `ServerHandler` that exposes the gateway's 3 meta-tools
//! (`search_tools` / `get_tool_details` / `call_tool`) to MCP clients over stdio.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use gateway::GatewayState;

/// The downstream MCP server. Holds shared gateway state plus the default `top_k`
/// used when a `search_tools` call omits it (sourced from `[retrieval].top_k`).
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<GatewayState>,
    default_top_k: usize,
}

impl GatewayServer {
    pub fn new(state: Arc<GatewayState>, default_top_k: usize) -> Self {
        Self { state, default_top_k }
    }
}

fn object_schema(json: serde_json::Value) -> Arc<serde_json::Map<String, serde_json::Value>> {
    match json {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => Arc::new(serde_json::Map::new()),
    }
}

/// The fixed set of 3 meta-tools exposed to clients. Stable regardless of upstreams.
pub fn meta_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "search_tools",
            "Search aggregated upstream tools by natural-language query; returns candidate \
             tool summaries (qualified name + description).",
            object_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural-language query." },
                    "top_k": { "type": "integer", "description": "Max results to return." }
                },
                "required": ["query"]
            })),
        ),
        Tool::new(
            "get_tool_details",
            "Get the full definition (description + input schema) of one tool by its \
             qualified name (e.g. \"github__create_issue\").",
            object_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Qualified tool name." }
                },
                "required": ["name"]
            })),
        ),
        Tool::new(
            "call_tool",
            "Execute one upstream tool by its qualified name, forwarding `arguments`.",
            object_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Qualified tool name." },
                    "arguments": { "type": "object", "description": "Tool arguments." }
                },
                "required": ["name"]
            })),
        ),
    ]
}

impl ServerHandler for GatewayServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(meta_tools()))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // 派发在 Task 3 实现；先返回占位错误以便骨架编译。
        Ok(CallToolResult::error(vec![Content::text("not implemented")]))
    }
}
```

> 注：`get_info` 只 `enable_tools()`，不 `enable_tool_list_changed()`——下游的 3 元工具恒定不变，
> list_changed 是 *上游→网关* 的事，不向下游客户端转发（这是对 spec §3.1 的工程化收敛）。
> `ServerInfo::new(...).with_server_info(Implementation::from_build_env())` 与 testkit 一致。

- [ ] **Step 5: 运行测试确认通过 + clippy**

Run: `cargo test -p downstream && cargo clippy -p downstream --all-targets -- -D warnings`
Expected: PASS（含 `meta_tools_are_exactly_the_three`），clippy 零告警。

- [ ] **Step 6: Commit**

```bash
git add crates/downstream Cargo.toml
git commit -m "feat(downstream): scaffold GatewayServer + meta_tools()

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: e2e 夹具 + `list_tools` 恒返回 3 元工具

**Files:**
- Create: `crates/downstream/tests/common/mod.rs`
- Create: `crates/downstream/tests/server.rs`

- [ ] **Step 1: 写 e2e 夹具**

`crates/downstream/tests/common/mod.rs`：

```rust
//! Shared e2e harness: run a GatewayServer over an in-memory duplex and return a
//! connected rmcp test client, plus helpers to attach a mock upstream.

use std::sync::Arc;

use gateway::GatewayState;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;

use downstream::GatewayServer;

/// Spawn a GatewayServer (over duplex) with the given state; return a connected client.
/// The server task is detached; the client drives the test.
pub async fn connect_to_gateway(
    state: Arc<GatewayState>,
    default_top_k: usize,
) -> RunningService<RoleClient, ()> {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = GatewayServer::new(state, default_top_k);
    tokio::spawn(async move {
        if let Ok(svc) = server.serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    ().serve(client_io).await.expect("client connects")
}
```

> 注：`tests/common/mod.rs` 作为子模块被各测试文件 `mod common;` 引入（cargo 不把 `mod.rs` 当独立 test target）。

- [ ] **Step 2: 写 list_tools 集成测试（失败）**

`crates/downstream/tests/server.rs`：

```rust
mod common;

use std::sync::Arc;

use gateway::GatewayState;
use rmcp::ServiceExt;

#[tokio::test]
async fn list_tools_returns_exactly_the_three_metatools() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let client = common::connect_to_gateway(state, 8).await;

    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["call_tool", "get_tool_details", "search_tools"]);

    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p downstream --test server list_tools_returns_exactly -- --nocapture`
Expected: 编译失败或断言不通过（夹具/导出未就绪），先确认红。

- [ ] **Step 4: 让其编译通过**

`GatewayServer` 与 `meta_tools` 已在 Task 1 实现且 `list_tools` 已返回 3 元工具；本 task 只新增测试与夹具，无生产代码改动。若 `GatewayState::new` 当前签名为 `Result<Self,String>`，`.unwrap()` 可用。

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p downstream --test server`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/downstream/tests
git commit -m "test(downstream): e2e harness + list_tools returns 3 meta-tools

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: `GatewayServer::call_tool` 派发 + `isError` 映射

**Files:**
- Modify: `crates/downstream/src/lib.rs`（实现 `call_tool`）
- Modify: `crates/downstream/tests/common/mod.rs`（加挂 mock 上游的 helper）
- Modify: `crates/downstream/tests/server.rs`（派发测试）

- [ ] **Step 1: 夹具加「挂一个 mock 上游并重建」helper**

`crates/downstream/tests/common/mod.rs` 追加：

```rust
use upstream::testkit::MockUpstream;
use upstream::connection::UpstreamHandle;

/// Attach a MockUpstream (echo/greet/slow) into the state's registry under `name`,
/// then rebuild the snapshot so its tools are searchable/callable.
pub async fn attach_mock(state: &GatewayState, name: &str) {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = MockUpstream::new().serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    state.registry().insert(std::sync::Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}
```

> `tests/common/mod.rs` 顶部相应加 `use upstream::...`（dev-dep 已含 testkit feature）。

- [ ] **Step 2: 写派发集成测试（失败）**

`crates/downstream/tests/server.rs` 追加：

```rust
use rmcp::model::CallToolRequestParams;
use serde_json::json;

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

#[tokio::test]
async fn call_tool_dispatches_all_three_metatools() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;
    let client = common::connect_to_gateway(state, 8).await;

    // search_tools 找到 echo。
    let r = client
        .call_tool(CallToolRequestParams::new("search_tools").with_arguments(args(json!({"query":"echo"}))))
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    let text = r.content[0].as_text().unwrap().text.clone();
    assert!(text.contains("mock__echo"), "search result: {text}");

    // get_tool_details 返回 echo 定义。
    let r = client
        .call_tool(CallToolRequestParams::new("get_tool_details").with_arguments(args(json!({"name":"mock__echo"}))))
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("echo"));

    // call_tool 转发到上游 echo。
    let r = client
        .call_tool(CallToolRequestParams::new("call_tool").with_arguments(args(json!({
            "name": "mock__echo", "arguments": {"text": "hi"}
        }))))
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("hi"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn call_tool_unknown_meta_name_is_protocol_error() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let client = common::connect_to_gateway(state, 8).await;
    let err = client
        .call_tool(CallToolRequestParams::new("does_not_exist"))
        .await;
    assert!(err.is_err(), "unknown meta-tool must be a protocol error");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn call_tool_routes_missing_upstream_tool_to_iserror() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;
    let client = common::connect_to_gateway(state, 8).await;
    let r = client
        .call_tool(CallToolRequestParams::new("call_tool").with_arguments(args(json!({"name":"mock__nope"}))))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true)); // MetaError::ToolNotFound -> isError
    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p downstream --test server call_tool_dispatches -- --nocapture`
Expected: FAIL（当前 `call_tool` 返回 "not implemented"）。

- [ ] **Step 4: 实现派发**

替换 `crates/downstream/src/lib.rs` 中 `call_tool` 占位实现：

```rust
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let args = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "search_tools" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let top_k = args
                    .get("top_k")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(self.default_top_k);
                let snap = self.state.snapshot();
                let hits = metatools::search_tools(&snap, query, top_k);
                let json = serde_json::to_string(&hits)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            "get_tool_details" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let snap = self.state.snapshot();
                match metatools::get_tool_details(&snap, name) {
                    Some(def) => {
                        let json = serde_json::to_string(def)
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    None => Ok(CallToolResult::error(vec![Content::text(format!(
                        "no such tool: {name}"
                    ))])),
                }
            }
            "call_tool" => {
                let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "missing required 'name'",
                    )]));
                };
                let inner = args.get("arguments").and_then(|v| v.as_object()).cloned();
                let snap = self.state.snapshot();
                match metatools::call_tool(&snap, self.state.registry(), name, inner).await {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
```

- [ ] **Step 5: 运行确认通过 + clippy**

Run: `cargo test -p downstream && cargo clippy -p downstream --all-targets -- -D warnings`
Expected: 全部 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/downstream
git commit -m "feat(downstream): dispatch call_tool to the 3 meta-tools; MetaError -> isError

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: `gateway` 类型化错误 + 重建遥测 + 并发 ingest（修死锁）

**Files:**
- Modify: `crates/upstream/src/connection.rs`（加 `call_timeout()` getter）
- Modify: `crates/gateway/src/lib.rs`（`GatewayError`、`RebuildSummary`、并发 ingest + per-ingest 超时）
- Test: `crates/gateway/tests/rebuild.rs`（现有，需更新返回类型 + 加 hung-during-rebuild 测试）

- [ ] **Step 1: 给 `UpstreamHandle` 加 `call_timeout()` getter**

`crates/upstream/src/connection.rs`，`server()` 方法旁：

```rust
    /// The per-call timeout configured for this handle (used by the gateway to bound ingest).
    pub fn call_timeout(&self) -> std::time::Duration {
        self.call_timeout
    }
```

- [ ] **Step 2: 写 hung-during-rebuild 失败测试**

为避免在 `connect`（initialize）阶段就挂起，测试用一个**能正常 initialize、但 `list_tools` 永久 sleep**
的最小内联 server——这样 `connect` 立即成功，挂起只发生在 `ingest_into`→`list_all_tools`，正好由
per-ingest 超时兜住。`crates/gateway/tests/rebuild.rs` 追加：

```rust
// 一个 initialize 正常、但 list_tools 永不返回的上游：用于验证 per-ingest 超时。
#[derive(Clone)]
struct StalledListServer;

impl rmcp::ServerHandler for StalledListServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(rmcp::model::Implementation::from_build_env())
    }

    async fn list_tools(
        &self,
        _r: Option<rmcp::model::PaginatedRequestParams>,
        _c: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        // 远超任何合理超时：模拟“已连接但静默”的上游。
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        Ok(rmcp::model::ListToolsResult::with_all_items(vec![]))
    }
}

#[tokio::test]
async fn rebuild_isolates_an_upstream_that_hangs_during_ingest() {
    use std::time::Duration;
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = StalledListServer.serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    // connect (initialize) 立即成功；ingest 会卡在 list_tools，被 80ms 超时跳过。
    let handle = upstream::connection::UpstreamHandle::connect("hung", client_io)
        .await
        .unwrap()
        .with_call_timeout(Duration::from_millis(80));

    let state = gateway::GatewayState::new("bm25").unwrap();
    state.registry().insert(std::sync::Arc::new(handle));

    // 整体重建必须远小于默认 30s 完成（这里给 5s 上限）。
    let summary = tokio::time::timeout(Duration::from_secs(5), state.rebuild_snapshot())
        .await
        .expect("rebuild must not hang on a stalled upstream")
        .unwrap();

    assert!(summary.ingested.is_empty());
    assert_eq!(summary.skipped.len(), 1);
    assert_eq!(summary.skipped[0].0, "hung");
}
```

> `StalledListServer` 需要 `use rmcp::ServiceExt;`（`.serve`）——`rebuild.rs` 顶部已有则复用。

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p gateway --test rebuild rebuild_isolates_an_upstream_that_hangs -- --nocapture`
Expected: 编译失败（`rebuild_snapshot` 还返回 `Result<(),String>`，无 `summary.ingested`）。

- [ ] **Step 4: 实现类型化错误 + 遥测 + 并发 ingest**

`crates/gateway/src/lib.rs` 重写（保留 `new`/`registry`/`snapshot`/Send+Sync 测试，改 `new`/`rebuild_snapshot`）：

```rust
use std::sync::Arc;

use arc_swap::ArcSwap;
use catalog::Catalog;
use metatools::GatewaySnapshot;
use retrieval::build_strategy;
use tokio::sync::Mutex;
use upstream::registry::UpstreamRegistry;

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("unknown retrieval strategy: {0}")]
    Strategy(String),
}

/// Telemetry for one snapshot rebuild.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RebuildSummary {
    /// Upstreams whose tools were ingested into the new snapshot.
    pub ingested: Vec<String>,
    /// Upstreams skipped this rebuild, with a short reason (timeout / call error).
    pub skipped: Vec<(String, String)>,
}
```

`new` 改返回 `GatewayError`：

```rust
    pub fn new(strategy_name: &str) -> Result<Self, GatewayError> {
        let mut strat =
            build_strategy(strategy_name).map_err(|e| GatewayError::Strategy(e.to_string()))?;
        let empty = Catalog::new();
        strat.index(&empty);
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
            rebuild_lock: Arc::new(Mutex::new(())),
        })
    }
```

`rebuild_snapshot` 改并发 + per-ingest 超时 + 返回遥测：

```rust
    /// Rebuild the snapshot by ingesting every upstream's tools **concurrently**, each bounded
    /// by that handle's `call_timeout`. A slow/hung/failing upstream is isolated (recorded in
    /// `skipped`) and never blocks the others or the rebuild. Build-then-swap keeps reads
    /// lock-free; `rebuild_lock` serializes overlapping rebuilds (last-store-wins).
    pub async fn rebuild_snapshot(&self) -> Result<RebuildSummary, GatewayError> {
        let _guard = self.rebuild_lock.lock().await;

        let mut set = tokio::task::JoinSet::new();
        for name in self.registry.server_names() {
            if let Some(handle) = self.registry.get(&name) {
                let timeout = handle.call_timeout();
                set.spawn(async move {
                    let mut local = Catalog::new();
                    let outcome =
                        tokio::time::timeout(timeout, handle.ingest_into(&mut local)).await;
                    (name, outcome, local)
                });
            }
        }

        let mut summary = RebuildSummary::default();
        let mut catalog = Catalog::new();
        while let Some(joined) = set.join_next().await {
            let (name, outcome, local) = joined.expect("ingest task panicked");
            match outcome {
                Err(_elapsed) => summary.skipped.push((name, "ingest timed out".to_string())),
                Ok(Err(e)) => summary.skipped.push((name, e.to_string())),
                Ok(Ok(_dupes)) => {
                    for tool in local.iter() {
                        catalog.upsert(tool.clone());
                    }
                    summary.ingested.push(name);
                }
            }
        }
        summary.ingested.sort();
        summary.skipped.sort();

        let mut strat = build_strategy(&self.strategy_name)
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        strat.index(&catalog);
        self.snapshot
            .store(Arc::new(GatewaySnapshot::new(catalog, strat)));
        Ok(summary)
    }
```

> 各上游命名空间互不相交，故按 `qualified_name` `upsert` 合并无冲突；intra-server 去重仍由
> `ingest_into`（内部 `ingest_tools` first-dupe-wins）在各自局部 catalog 内完成。
> 用 `tokio::task::JoinSet`（tokio `rt`，workspace 已启用），无需引入 `futures`。

- [ ] **Step 5: 更新现有 rebuild 测试的返回类型**

把 `crates/gateway/tests/rebuild.rs` 中所有 `rebuild_snapshot().await.unwrap()`（原 `()`）保持不变即可（`unwrap()` 适配新 `Result`）；若有断言依赖返回值需改为读 `RebuildSummary`。`GatewayState::new(...)` 的 `.unwrap()` 仍可用。检查 `crates/gateway/src/lib.rs` 内联单测（`gateway_state_is_send_sync`）无需改。

- [ ] **Step 6: 运行确认通过**

Run: `cargo test -p gateway -p upstream && cargo clippy -p gateway -p upstream --all-targets --all-features -- -D warnings`
Expected: 全 PASS（含新 hung-during-rebuild 测试），clippy 干净。

- [ ] **Step 7: Commit**

```bash
git add crates/gateway crates/upstream/src/connection.rs
git commit -m "feat(gateway): typed GatewayError + RebuildSummary; concurrent ingest with per-ingest timeout

Fixes the rebuild-deadlock hazard: a hung upstream can no longer stall ingest or
starve the rebuild lock. Adds UpstreamHandle::call_timeout() getter.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: `UpstreamClientHandler` + connect 句柄迁移（为 list_changed 铺路）

**Files:**
- Modify: `crates/upstream/src/connection.rs`（`RebuildTrigger`、`UpstreamClientHandler`、`client` 字段类型、`connect`/`connect_with_trigger`）
- Test: `crates/upstream/tests/integration.rs`（连接迁移不破坏 ingest/call）

- [ ] **Step 1: 写迁移不破坏功能的失败测试**

`crates/upstream/tests/integration.rs` 追加（沿用该文件已有 `use`：`Catalog`、`UpstreamHandle`、`MockUpstream`、`ServiceExt`）：

```rust
#[tokio::test]
async fn connect_with_trigger_preserves_ingest_and_call() {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        MockUpstream::new().serve(server_io).await.unwrap().waiting().await.unwrap();
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
    let handle = UpstreamHandle::connect_with_trigger("mock", client_io, Some(tx))
        .await
        .unwrap();

    let mut cat = Catalog::new();
    handle.ingest_into(&mut cat).await.unwrap();
    assert!(cat.get("mock__echo").is_some());

    let r = handle
        .call_tool("echo", serde_json::json!({"text":"hi"}).as_object().cloned())
        .await
        .unwrap();
    assert!(r.content[0].as_text().unwrap().text.contains("hi"));

    // 未发生 list_changed，trigger 通道应为空。
    assert!(rx.try_recv().is_err());
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p upstream --all-features --test integration connect_with_trigger_preserves -- --nocapture`
Expected: 编译失败（`connect_with_trigger` 未定义）。

- [ ] **Step 3: 实现 handler + 迁移 connect**

`crates/upstream/src/connection.rs` 顶部 import 增加：

```rust
use rmcp::service::{NotificationContext, RoleClient, RunningService};
use rmcp::transport::IntoTransport;
use rmcp::{ClientHandler, ServiceExt};
```

（删除原先重复的 `use rmcp::service::{RoleClient, RunningService};` 与 `use rmcp::ServiceExt;`，合并为上面。）

新增类型（放在 `UpstreamHandle` 之前）：

```rust
/// A bounded channel the gateway drains to rebuild its snapshot. The handler sends the
/// upstream's name on each `tools/list_changed`; a full channel is fine (worker coalesces).
pub type RebuildTrigger = tokio::sync::mpsc::Sender<String>;

/// rmcp client handler installed on every upstream connection. On `tools/list_changed`
/// it nudges the rebuild trigger; with `trigger: None` it is a no-op (used by in-memory tests).
#[derive(Clone)]
pub struct UpstreamClientHandler {
    server: String,
    trigger: Option<RebuildTrigger>,
}

impl ClientHandler for UpstreamClientHandler {
    async fn on_tool_list_changed(&self, _ctx: NotificationContext<RoleClient>) {
        if let Some(tx) = &self.trigger {
            let _ = tx.try_send(self.server.clone());
        }
    }
}
```

`UpstreamHandle.client` 字段类型改为带 handler 的具体类型：

```rust
pub struct UpstreamHandle {
    server: String,
    client: RunningService<RoleClient, UpstreamClientHandler>,
    call_timeout: std::time::Duration,
}
```

`connect` 改为委派；新增 `connect_with_trigger`（泛型覆盖 duplex 与 `TokioChildProcess`）：

```rust
impl UpstreamHandle {
    /// Connect over any transport with NO list_changed trigger (in-memory tests).
    pub async fn connect<T, E, A>(server: &str, transport: T) -> Result<Self, UpstreamError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::connect_with_trigger(server, transport, None).await
    }

    /// Connect and install an `UpstreamClientHandler` carrying `trigger`.
    pub async fn connect_with_trigger<T, E, A>(
        server: &str,
        transport: T,
        trigger: Option<RebuildTrigger>,
    ) -> Result<Self, UpstreamError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let handler = UpstreamClientHandler {
            server: server.to_string(),
            trigger,
        };
        let client = handler
            .serve(transport)
            .await
            .map_err(|e| UpstreamError::Connect {
                server: server.to_string(),
                source: Box::new(e),
            })?;
        Ok(Self {
            server: server.to_string(),
            client,
            call_timeout: std::time::Duration::from_secs(30),
        })
    }
    // ... with_call_timeout / call_timeout / server / ingest_into / call_tool / shutdown 不变 ...
}
```

> `connect` 的泛型从 `AsyncRead+AsyncWrite` 改为 `IntoTransport<RoleClient,E,A>`：duplex 经
> async-rw 适配、`TokioChildProcess` 经 Transport 适配，都满足；已有 2 参调用点（`connect("mock", io)`）
> 仍按推断编译，无需改动。

- [ ] **Step 4: 运行确认通过（含既有全部 upstream 测试）**

Run: `cargo test -p upstream --all-features && cargo clippy -p upstream --all-targets --all-features -- -D warnings`
Expected: 全 PASS（既有 6+6 测试 + 新迁移测试），clippy 干净。

- [ ] **Step 5: 全工作区回归（句柄类型变更不影响 metatools/gateway/downstream）**

Run: `cargo test --all-features`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/upstream
git commit -m "feat(upstream): UpstreamClientHandler + connect_with_trigger (list_changed plumbing)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6: `config` 增加 `[server]` 段

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: 写失败测试**

`crates/config/src/lib.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn server_section_parses_and_defaults_to_stdio() {
        // 省略 [server] -> 默认 stdio = true。
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.server.stdio);

        // 显式给出。
        let cfg = Config::from_toml_str("[server]\nstdio = true\n").unwrap();
        assert!(cfg.server.stdio);

        // 未知键被拒绝（ServerConfig 无 flatten，可 deny_unknown_fields）。
        assert!(Config::from_toml_str("[server]\nbogus = 1\n").is_err());
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p config server_section_parses -- --nocapture`
Expected: 编译失败（`cfg.server` / `ServerConfig` 不存在）。

- [ ] **Step 3: 实现 `ServerConfig`**

`crates/config/src/lib.rs`：`Config` 结构体加字段：

```rust
    #[serde(default)]
    pub server: ServerConfig,
```

新增类型（放在 `RetrievalConfig` 之后）：

```rust
/// `[server]` section: which downstream transport(s) to serve. HTTP arrives in M1-C.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Serve the 3 meta-tools over a stdio MCP server.
    pub stdio: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { stdio: true }
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p config && cargo clippy -p config --all-targets -- -D warnings`
Expected: 全 PASS（含既有 13 + 新测试）。

- [ ] **Step 5: Commit**

```bash
git add crates/config
git commit -m "feat(config): add [server] section (stdio, default true)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 7: `connect_all` + `connect_stdio_upstream`（真实子进程 + 降级启动）

**Files:**
- Create: `crates/upstream/src/connect.rs`
- Modify: `crates/upstream/src/lib.rs`（`mod connect;` + 重导出 + 依赖 `config`）
- Modify: `crates/upstream/Cargo.toml`（加 `config` 依赖；加 testkit-gated 的 `[[bin]] mock-stdio`）
- Create: `crates/upstream/src/bin/mock-stdio.rs`（testkit-only：stdio 上跑 MockUpstream，供冒烟测试）
- Test: `crates/upstream/tests/integration.rs`（降级启动 + 子进程冒烟）

- [ ] **Step 1: 加依赖 + bin 目标**

`crates/upstream/Cargo.toml` `[dependencies]` 加：

```toml
config = { path = "../config" }
```

文件末尾加（testkit 时才编译该 bin，集成测试可经 `CARGO_BIN_EXE_mock-stdio` 引用）：

```toml
[[bin]]
name = "mock-stdio"
required-features = ["testkit"]
```

`crates/upstream/src/bin/mock-stdio.rs`：

```rust
//! Test-only: run the in-memory MockUpstream over real stdio, so the subprocess
//! connect path (`connect_stdio_upstream`) can be smoke-tested against a real child.
use rmcp::transport::stdio;
use rmcp::ServiceExt;
use upstream::testkit::MockUpstream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = MockUpstream::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

> 需要 `upstream::testkit` 为 crate 公共可见（已是 `pub mod testkit` 且 `#![cfg(any(test, feature="testkit"))]`）。

- [ ] **Step 2: 写失败测试（降级启动 + 子进程冒烟）**

`crates/upstream/tests/integration.rs` 追加：

```rust
use config::{UpstreamConfig, UpstreamTransport};
use upstream::connect::{connect_all, connect_stdio_upstream};
use upstream::registry::UpstreamRegistry;

fn stdio_cfg(name: &str, command: &str, args: Vec<String>) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        call_timeout_ms: 5_000,
        transport: UpstreamTransport::Stdio { command: command.to_string(), args, env_passthrough: vec![] },
    }
}

#[tokio::test]
async fn connect_all_degraded_start_isolates_bad_upstreams() {
    let registry = UpstreamRegistry::new();
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
    let cfgs = vec![
        stdio_cfg("bad1", "definitely-not-a-real-binary-xyzzy", vec![]),
        stdio_cfg("bad2", "definitely-not-a-real-binary-zzz", vec![]),
    ];
    let summary = connect_all(&registry, &cfgs, tx).await;
    assert!(summary.connected.is_empty());
    assert_eq!(summary.skipped.len(), 2);
    assert!(registry.server_names().is_empty());
}

#[tokio::test]
async fn connect_stdio_upstream_smoke_spawns_real_child() {
    let exe = env!("CARGO_BIN_EXE_mock-stdio");
    let cfg = stdio_cfg("child", exe, vec![]);
    let handle = connect_stdio_upstream(&cfg, None).await.expect("spawn + connect");

    let mut cat = catalog::Catalog::new();
    handle.ingest_into(&mut cat).await.unwrap();
    assert!(cat.get("child__echo").is_some());

    std::sync::Arc::new(handle); // drop cancels the child service
}
```

> `crates/upstream/tests/integration.rs` 顶部如未引入 `catalog`，加 `use catalog;`（dev 依赖已含）。

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p upstream --all-features --test integration connect_all_degraded -- --nocapture`
Expected: 编译失败（`upstream::connect` 模块不存在）。

- [ ] **Step 4: 实现 `connect.rs`**

`crates/upstream/src/connect.rs`：

```rust
//! Eager, degraded-start connection of all configured upstreams (real stdio children).

use std::sync::Arc;
use std::time::Duration;

use config::{UpstreamConfig, UpstreamTransport};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};

use crate::connection::{RebuildTrigger, UpstreamError, UpstreamHandle};
use crate::registry::UpstreamRegistry;

/// Outcome of `connect_all`: which upstreams connected vs. were skipped (with reason).
pub struct ConnectSummary {
    pub connected: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// Spawn one stdio upstream child and connect to it, applying `call_timeout_ms` and
/// installing the list_changed handler carrying `trigger`.
pub async fn connect_stdio_upstream(
    cfg: &UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError> {
    let (command, args, env_passthrough) = match &cfg.transport {
        UpstreamTransport::Stdio { command, args, env_passthrough } => (command, args, env_passthrough),
    };
    let cmd = tokio::process::Command::new(command).configure(|c| {
        c.args(args);
        for key in env_passthrough {
            if let Ok(val) = std::env::var(key) {
                c.env(key, val);
            }
        }
    });
    let transport = TokioChildProcess::new(cmd).map_err(|e| UpstreamError::Connect {
        server: cfg.name.clone(),
        source: Box::new(e),
    })?;
    let handle = UpstreamHandle::connect_with_trigger(&cfg.name, transport, trigger)
        .await?
        .with_call_timeout(Duration::from_millis(cfg.call_timeout_ms));
    Ok(handle)
}

/// Connect every configured upstream eagerly. Degraded start: a connect failure is
/// `warn!`-logged and recorded in `skipped`; successful handles are inserted into `registry`.
pub async fn connect_all(
    registry: &UpstreamRegistry,
    upstreams: &[UpstreamConfig],
    trigger: RebuildTrigger,
) -> ConnectSummary {
    let mut summary = ConnectSummary { connected: vec![], skipped: vec![] };
    for cfg in upstreams {
        match connect_stdio_upstream(cfg, Some(trigger.clone())).await {
            Ok(handle) => {
                registry.insert(Arc::new(handle));
                summary.connected.push(cfg.name.clone());
            }
            Err(e) => {
                tracing::warn!(upstream = %cfg.name, error = %e, "connect failed; skipping");
                summary.skipped.push((cfg.name.clone(), e.to_string()));
            }
        }
    }
    summary
}
```

`crates/upstream/src/lib.rs` 加模块 + 重导出：

```rust
pub mod connect;
```

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p upstream --all-features && cargo clippy -p upstream --all-targets --all-features -- -D warnings`
Expected: 全 PASS（含降级 + 子进程冒烟），clippy 干净。

- [ ] **Step 6: Commit**

```bash
git add crates/upstream
git commit -m "feat(upstream): connect_all + connect_stdio_upstream (real child, degraded start)

Adds a testkit-only mock-stdio bin for the subprocess smoke test.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 8: testkit `RevealingMockUpstream`（运行期揭示新工具 + 发 list_changed）

**Files:**
- Modify: `crates/upstream/src/testkit.rs`（新增 `RevealingMockUpstream`，手写 `ServerHandler`）
- Test: `crates/upstream/tests/integration.rs`（reveal 后工具表增长）

- [ ] **Step 1: 写失败测试**

`crates/upstream/tests/integration.rs` 追加：

```rust
use upstream::testkit::RevealingMockUpstream;

#[tokio::test]
async fn revealing_mock_grows_its_tool_list_after_reveal() {
    use rmcp::model::CallToolRequestParams;
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        RevealingMockUpstream::new().serve(server_io).await.unwrap().waiting().await.unwrap();
    });
    let client = ().serve(client_io).await.unwrap();

    let before: Vec<String> = client.list_all_tools().await.unwrap().iter().map(|t| t.name.to_string()).collect();
    assert!(before.contains(&"echo".to_string()));
    assert!(before.contains(&"reveal".to_string()));
    assert!(!before.contains(&"late_tool".to_string()));

    client.call_tool(CallToolRequestParams::new("reveal")).await.unwrap();

    let after: Vec<String> = client.list_all_tools().await.unwrap().iter().map(|t| t.name.to_string()).collect();
    assert!(after.contains(&"late_tool".to_string()), "reveal must expose late_tool: {after:?}");

    client.cancel().await.unwrap();
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p upstream --all-features --test integration revealing_mock_grows -- --nocapture`
Expected: 编译失败（`RevealingMockUpstream` 不存在）。

- [ ] **Step 3: 实现 `RevealingMockUpstream`**

`crates/upstream/src/testkit.rs` 追加（手写 `ServerHandler`，区别于宏驱动的 `MockUpstream`）：

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};

fn empty_object_schema() -> Arc<serde_json::Map<String, serde_json::Value>> {
    match serde_json::json!({"type": "object"}) {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => Arc::new(serde_json::Map::new()),
    }
}

/// A mock upstream whose tool list changes at runtime: it starts with `echo` + `reveal`,
/// and calling `reveal` exposes `late_tool` AND emits `tools/list_changed` to the client.
/// Used to drive the gateway's list_changed refresh path end-to-end.
#[derive(Clone, Default)]
pub struct RevealingMockUpstream {
    revealed: Arc<AtomicBool>,
}

impl RevealingMockUpstream {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ServerHandler for RevealingMockUpstream {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::from_build_env())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = vec![
            Tool::new("echo", "Echo the input text", empty_object_schema()),
            Tool::new(
                "reveal",
                "Reveal late_tool and emit tools/list_changed",
                empty_object_schema(),
            ),
        ];
        if self.revealed.load(Ordering::SeqCst) {
            tools.push(Tool::new(
                "late_tool",
                "A tool revealed at runtime",
                empty_object_schema(),
            ));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match request.name.as_ref() {
            "echo" => {
                let text = request
                    .arguments
                    .as_ref()
                    .and_then(|a| a.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            "reveal" => {
                self.revealed.store(true, Ordering::SeqCst);
                let _ = ctx.peer.notify_tool_list_changed().await;
                Ok(CallToolResult::success(vec![Content::text("revealed")]))
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
}
```

> `testkit.rs` 顶部已 `use rmcp::model::{..., Implementation, ServerCapabilities, ServerInfo}` 与
> `use rmcp::{..., ServerHandler}`（`MockUpstream` 已用）；如缺则补全上面新增类型的 import。

- [ ] **Step 4: 运行确认通过 + clippy**

Run: `cargo test -p upstream --all-features && cargo clippy -p upstream --all-targets --all-features -- -D warnings`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/upstream/src/testkit.rs crates/upstream/tests/integration.rs
git commit -m "test(upstream): RevealingMockUpstream that reveals a tool + emits list_changed

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 9: `gateway::run_rebuild_worker`（drain + 合并 + 重建）

**Files:**
- Modify: `crates/gateway/src/lib.rs`（`run_rebuild_worker`）
- Test: `crates/gateway/tests/rebuild.rs`（触发后快照含新工具）

- [ ] **Step 1: 写失败测试**

`crates/gateway/tests/rebuild.rs` 追加（沿用该文件已有 `use`；缺则补 `MockUpstream`/`UpstreamHandle`/`ServiceExt`）：

```rust
#[tokio::test]
async fn rebuild_worker_rebuilds_when_triggered() {
    use std::time::Duration;
    let state = gateway::GatewayState::new("bm25").unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(8);
    let worker = tokio::spawn(gateway::run_rebuild_worker(state.clone(), rx));

    // 挂一个 mock 上游（但先不触发重建）。
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        MockUpstream::new().serve(server_io).await.unwrap().waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect("mock", client_io).await.unwrap();
    state.registry().insert(std::sync::Arc::new(handle));

    // 触发前：快照为空（搜不到）。
    assert!(metatools::search_tools(&state.snapshot(), "echo", 5).is_empty());

    // 触发一次重建。
    tx.send("mock".to_string()).await.unwrap();

    // 轮询直到快照反映 mock 工具。
    let mut found = false;
    for _ in 0..100 {
        if metatools::search_tools(&state.snapshot(), "echo", 5)
            .iter()
            .any(|s| s.name == "mock__echo")
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(found, "worker should rebuild snapshot to include mock__echo");

    drop(tx); // 关闭通道 -> worker 退出
    let _ = worker.await;
}
```

> `crates/gateway/Cargo.toml` 的 dev-deps 已含 `upstream`(testkit)、`rmcp`(client/server/macros/transport-io)、`tokio`(full)；`metatools` 是普通依赖，测试可直接用。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p gateway --test rebuild rebuild_worker_rebuilds -- --nocapture`
Expected: 编译失败（`run_rebuild_worker` 不存在）。

- [ ] **Step 3: 实现 `run_rebuild_worker`**

`crates/gateway/src/lib.rs`（`impl GatewayState` 之后，模块级函数）：

```rust
/// Drain `rx` and rebuild the snapshot once per burst (coalescing consecutive triggers).
/// Exits when the channel closes (all `RebuildTrigger` senders dropped). `serve` spawns this.
pub async fn run_rebuild_worker(
    state: GatewayState,
    mut rx: tokio::sync::mpsc::Receiver<String>,
) {
    while rx.recv().await.is_some() {
        // Coalesce any other pending triggers so a burst yields a single rebuild.
        while rx.try_recv().is_ok() {}
        match state.rebuild_snapshot().await {
            Ok(s) => tracing::info!(
                ingested = ?s.ingested,
                skipped = ?s.skipped,
                "snapshot rebuilt (list_changed)"
            ),
            Err(e) => tracing::warn!(error = %e, "rebuild failed"),
        }
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p gateway --all-features && cargo clippy -p gateway --all-targets --all-features -- -D warnings`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/gateway
git commit -m "feat(gateway): run_rebuild_worker (coalescing list_changed-driven rebuilds)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 10: `mcpgw serve` 命令（装配全链路）

**Files:**
- Modify: `crates/mcpgw/Cargo.toml`（加 `gateway`/`upstream`/`downstream`/`rmcp`/`tokio`/`tracing`；dev: testkit）
- Modify: `crates/mcpgw/src/main.rs`（`Serve` 子命令 + `run_serve`；把 catalog 读取下移到 search/get-details 分支）

- [ ] **Step 1: 加依赖**

`crates/mcpgw/Cargo.toml`：

```toml
[dependencies]
catalog = { path = "../catalog" }
retrieval = { path = "../retrieval" }
config = { path = "../config" }
gateway = { path = "../gateway" }
upstream = { path = "../upstream" }
downstream = { path = "../downstream" }
serde_json = { workspace = true }
clap = { workspace = true }
rmcp = { workspace = true, features = ["server", "transport-io"] }
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync"] }
tracing = { workspace = true }

[dev-dependencies]
upstream = { path = "../upstream", features = ["testkit"] }
metatools = { path = "../metatools" }
rmcp = { workspace = true, features = ["client", "server", "transport-io"] }
tokio = { workspace = true, features = ["full"] }
```

- [ ] **Step 2: 写 CLI 解析 + serve 烟测（失败）**

`crates/mcpgw/src/main.rs` 的 `#[cfg(test)] mod tests`（如无则新建）追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_parses_serve_subcommand() {
        let cli = Cli::try_parse_from(["mcpgw", "serve"]).unwrap();
        assert!(matches!(cli.command, Command::Serve));
    }

    #[tokio::test]
    async fn run_serve_builds_initial_snapshot_with_no_upstreams() {
        // 空配置：0 上游、stdio 默认开。run_serve 的“启动到 serve 之前”应成功建空快照。
        // 这里只验证装配前半段不报错：抽出的 prepare_state 在无上游时返回可用 state。
        let cfg = config::Config::default_from_empty();
        let (state, _rx) = prepare_state(&cfg).await.expect("prepare ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5).is_empty());
    }
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p mcpgw cli_parses_serve -- --nocapture`
Expected: 编译失败（`Command::Serve` / `prepare_state` 不存在）。

- [ ] **Step 4: 实现 serve 装配**

`crates/mcpgw/src/main.rs`：

`Command` 枚举加：

```rust
    /// Run the live MCP gateway server (stdio): aggregate upstreams, expose the 3 meta-tools.
    Serve,
```

把 `run` 顶部对 `cli.catalog` 的无条件读取**下移**进 `Search`/`GetDetails` 分支（serve 不需要 catalog 文件）：

```rust
fn run(cli: Cli) -> Result<(), String> {
    let cfg = load_config(&cli.config)?;

    match cli.command {
        Command::Search { query, top_k } => {
            let catalog = load_catalog(&cli.catalog)?;
            let mut strat = build_strategy(&cfg.retrieval.strategy).map_err(|e| e.to_string())?;
            strat.index(&catalog);
            let k = top_k.unwrap_or(cfg.retrieval.top_k);
            // ... 原 search 逻辑不变 ...
        }
        Command::GetDetails { name } => {
            let catalog = load_catalog(&cli.catalog)?;
            match catalog.get(&name) {
                Some(tool) => println!("{}", serde_json::to_string_pretty(tool).unwrap()),
                None => return Err(format!("no such tool: {name}")),
            }
        }
        Command::Serve => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(run_serve(cfg))?;
        }
    }
    Ok(())
}

fn load_catalog(path: &std::path::Path) -> Result<Catalog, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("read catalog {path:?}: {e}"))?;
    Catalog::from_json_str(&json).map_err(|e| e.to_string())
}
```

新增 serve 实现（用 `gateway`/`upstream`/`downstream`）：

```rust
use std::sync::Arc;

/// Build gateway state, connect upstreams, build the initial snapshot, and return the
/// state plus the rebuild-trigger receiver for the worker. Split out so it is unit-testable.
async fn prepare_state(
    cfg: &config::Config,
) -> Result<(Arc<gateway::GatewayState>, tokio::sync::mpsc::Receiver<String>), String> {
    let state = Arc::new(
        gateway::GatewayState::new(&cfg.retrieval.strategy).map_err(|e| e.to_string())?,
    );
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let csum = upstream::connect::connect_all(state.registry(), &cfg.upstreams, tx).await;
    tracing::info!(connected = ?csum.connected, skipped = ?csum.skipped, "upstreams connected");
    let rsum = state.rebuild_snapshot().await.map_err(|e| e.to_string())?;
    tracing::info!(ingested = ?rsum.ingested, skipped = ?rsum.skipped, "initial snapshot built");
    Ok((state, rx))
}

async fn run_serve(cfg: config::Config) -> Result<(), String> {
    use rmcp::transport::stdio;
    use rmcp::ServiceExt;

    if !cfg.server.stdio {
        return Err("only the stdio server is supported in this build".to_string());
    }
    let (state, rx) = prepare_state(&cfg).await?;

    // list_changed-driven rebuild worker.
    tokio::spawn(gateway::run_rebuild_worker((*state).clone(), rx));

    let server = downstream::GatewayServer::new(state.clone(), cfg.retrieval.top_k);
    let service = server.serve(stdio()).await.map_err(|e| e.to_string())?;
    service.waiting().await.map_err(|e| e.to_string())?;

    // Best-effort graceful shutdown of upstream children.
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            if let Ok(h) = Arc::try_unwrap(handle) {
                h.shutdown().await;
            }
        }
    }
    Ok(())
}
```

`main.rs` 顶部 import 增加：`use metatools` 不需要（serve 用 gateway/downstream）；测试模块用到 `metatools::search_tools`，故在 `[dev-dependencies]` 加 `metatools = { path = "../metatools" }` 或在测试里改用 `state.snapshot()` 后经 downstream。简洁起见 dev-dep 加 `metatools`。

> `(*state).clone()` 把 `Arc<GatewayState>` 解引用后克隆出一个 `GatewayState`（其内部都是 `Arc`，克隆廉价）交给 worker；`state` 自身仍 `Arc` 共享给 server。

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p mcpgw && cargo clippy -p mcpgw --all-targets -- -D warnings`
Expected: 全 PASS（CLI 解析 + prepare_state 烟测；既有 5 个 CLI 测试不回归）。

- [ ] **Step 6: 手动验证 serve 能起且随 stdin 关闭退出**

Run: `printf '' | cargo run -q -p mcpgw -- serve`
Expected: 进程启动（日志：connected=[] / 初始空快照），stdin EOF 后干净退出（exit 0），无 panic。

- [ ] **Step 7: Commit**

```bash
git add crates/mcpgw
git commit -m "feat(mcpgw): add 'serve' command wiring connect_all + rebuild worker + stdio GatewayServer

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 11: list_changed 全链路 e2e

**Files:**
- Modify: `crates/downstream/tests/common/mod.rs`（带 trigger 挂 `RevealingMockUpstream` + 起 worker）
- Modify: `crates/downstream/tests/server.rs`（端到端：reveal → 通知 → 重建 → search 命中）

- [ ] **Step 1: 夹具加「带 trigger 挂可揭示 mock + 起 worker」**

`crates/downstream/tests/common/mod.rs` 追加：

```rust
use upstream::testkit::RevealingMockUpstream;

/// Attach a RevealingMockUpstream WITH a list_changed trigger, spawn the gateway's rebuild
/// worker, and build the initial snapshot. Returns nothing; the worker lives for the test.
pub async fn attach_revealing_mock_with_worker(state: &Arc<GatewayState>, name: &str) {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(8);
    tokio::spawn(gateway::run_rebuild_worker((**state).clone(), rx));

    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = RevealingMockUpstream::new().serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = UpstreamHandle::connect_with_trigger(name, client_io, Some(tx))
        .await
        .unwrap();
    state.registry().insert(std::sync::Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}
```

- [ ] **Step 2: 写端到端测试（失败）**

`crates/downstream/tests/server.rs` 追加：

```rust
#[tokio::test]
async fn list_changed_refreshes_what_search_can_find() {
    use std::time::Duration;
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_revealing_mock_with_worker(&state, "mock").await;
    let client = common::connect_to_gateway(state.clone(), 8).await;

    // 初始：late_tool 尚未揭示，搜不到。
    let r = client
        .call_tool(CallToolRequestParams::new("search_tools").with_arguments(args(json!({"query":"late_tool"}))))
        .await
        .unwrap();
    assert!(!r.content[0].as_text().unwrap().text.contains("mock__late_tool"));

    // 经网关调用上游的 reveal -> 上游发 tools/list_changed -> handler -> trigger -> worker 重建。
    client
        .call_tool(CallToolRequestParams::new("call_tool").with_arguments(args(json!({"name":"mock__reveal"}))))
        .await
        .unwrap();

    // 轮询直到 search 能看到新工具。
    let mut found = false;
    for _ in 0..100 {
        let r = client
            .call_tool(CallToolRequestParams::new("search_tools").with_arguments(args(json!({"query":"late_tool revealed runtime"}))))
            .await
            .unwrap();
        if r.content[0].as_text().unwrap().text.contains("mock__late_tool") {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(found, "after list_changed, search_tools should surface mock__late_tool");

    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p downstream --test server list_changed_refreshes -- --nocapture`
Expected: FAIL（夹具新 helper 未实现前编译失败；实现后若 list_changed 链路未通则断言失败）。

- [ ] **Step 4: 让其通过**

本 task 不需要新生产代码——所有链路（handler→trigger→worker→rebuild、reveal 通知）已在 Task 5/8/9 就位；只新增夹具 helper 与测试。`crates/downstream/Cargo.toml` dev-deps 已含 `upstream`(testkit)/`gateway`/`tokio`(full)；确认 `gateway` 在 dev-deps（普通 dep 即可，已有）。

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p downstream`
Expected: 全 PASS（含 list_changed e2e）。

- [ ] **Step 6: Commit**

```bash
git add crates/downstream/tests
git commit -m "test(downstream): e2e list_changed refresh (reveal -> notify -> rebuild -> search)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 12: L1–L4 文档 + 全工作区收口

**Files（按既有 `docs/README.md` 分级约定）:**
- Create: `docs/L2-components/downstream.md`、`docs/L3-details/downstream.md`、`docs/L4-api/downstream-lib.md`
- Create: `docs/L4-api/upstream-connect.md`
- Modify: `docs/L1-overview.md`（新增 downstream crate + `serve` 命令；刷新架构图与测试计数）
- Modify: `docs/L2-components/{upstream,gateway,config}.md`、`docs/L3-details/{upstream,gateway,config}.md`
- Modify: `docs/L4-api/{gateway-lib,upstream-connection,config-lib}.md`

- [ ] **Step 1: 新增 downstream 三级文档**

- `docs/L2-components/downstream.md`：职责（rmcp `ServerHandler`，对客户端暴露 3 元工具）、接口（`GatewayServer::new(state, default_top_k)`、`meta_tools()`）、依赖（gateway/metatools/rmcp）、不变量（`list_tools` 恒 3 个）。
- `docs/L3-details/downstream.md`：`call_tool` 派发表（search/get_details/call → metatools 函数）、`MetaError`→`isError` 与未知工具→`McpError` 的区别、`get_info` 只 `enable_tools()` 的理由、e2e 夹具说明。
- `docs/L4-api/downstream-lib.md`：逐 `pub` 项（`GatewayServer`、`meta_tools`、`ServerHandler` 三方法签名）。

- [ ] **Step 2: 更新 upstream 文档**

- `docs/L4-api/upstream-connect.md`（新）：`connect_all`、`connect_stdio_upstream`、`ConnectSummary`。
- `docs/L2/L3-upstream.md`：新增 `UpstreamClientHandler`/`RebuildTrigger`、`connect_with_trigger`、`connect_all`（降级启动）、`call_timeout()` getter、`RevealingMockUpstream`、testkit-only `mock-stdio` bin；修订 `connect` 的泛型从 AsyncRW 改为 `IntoTransport`。
- `docs/L4-api/upstream-connection.md`：补 `connect_with_trigger`/`call_timeout`/`UpstreamClientHandler`/`RebuildTrigger`。

- [ ] **Step 3: 更新 gateway 文档**

- `docs/L4-api/gateway-lib.md` + `docs/L2/L3-gateway.md`：`GatewayError`、`RebuildSummary`、`rebuild_snapshot` 新签名与**并发 ingest + per-ingest 超时**（呼应并删除此前 L3 标注的 ingest 死锁缺口）、`run_rebuild_worker`（合并语义）。

- [ ] **Step 4: 更新 config 文档**

- `docs/L4-api/config-lib.md` + `docs/L2/L3-config.md`：`[server]` 段（`ServerConfig{stdio}`，默认 true，`deny_unknown_fields`）。

- [ ] **Step 5: 更新 L1**

- `docs/L1-overview.md`：crate 列表加 `downstream`；架构段加“活网关：serve → connect_all → 下游 stdio server → list_changed worker”；`构建与测试` 计数刷新为全工作区实际值（运行下方命令取准数）；CLI 用法补 `mcpgw serve`。

- [ ] **Step 6: 全工作区收口验证**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净；clippy 零告警；全部测试 PASS（把各 crate 实际计数填进 L1）。

- [ ] **Step 7: Commit**

```bash
git add docs
git commit -m "docs(m1b2): L1-L4 for downstream + serve; update upstream/gateway/config layers

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 成功标准核对（M1-B 整体 / 见 spec §1）

- [ ] 聚合 ≥2 上游，下游 `tools/list` 恒 3 元工具 —— Task 2（list_tools=3）+ Task 7（connect_all 多上游）。
- [ ] stdio 下游跑通 `search → inspect → execute` —— Task 3（派发 e2e）。
- [ ] `list_changed` 后 `search_tools` 能搜到变更工具 —— Task 11（全链路 e2e）。
- [ ] 单上游崩溃/挂起不拖垮其余（超时 + 隔离） —— Task 4（hung-during-rebuild）+ Task 7（degraded start）。
- [ ] `mcpgw serve` 读配置→连上游→起 server→随 stdin 关闭退出 —— Task 10。
- [ ] 修复 `m1b2-ingest-timeout` 死锁 —— Task 4（并发 ingest + per-ingest 超时）。

## 收尾

全部 task 完成且 `cargo test --all-features` / `clippy -D warnings` / `fmt --check` 全绿后，调用
**superpowers:finishing-a-development-branch** 把 `feat/m1b2-downstream` 合并回 master（用户在本地 `--no-ff` 合并）。
更新 roadmap M1 备注：M1-B.2 ✅。`m1b2-cancel-note` 在真实子进程上若确认 rmcp 优雅丢弃陈旧响应则关闭，否则降级为
M1-C 待办。
