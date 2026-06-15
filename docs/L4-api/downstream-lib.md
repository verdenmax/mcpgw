# L4 — `crates/downstream/src/lib.rs` API

源文件：`crates/downstream/src/lib.rs`。把网关的 3 个固定元工具经 rmcp `ServerHandler`（stdio）暴露给 MCP 客户端，并在每次调用边界构造一条 `observe::CallRecord` 扇出到注入的 sink（**仅元数据**）。

## `struct GatewayServer`
```rust
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<gateway::GatewayState>,        // 私有，共享网关状态
    default_top_k: usize,                     // 私有，search_tools 省略 top_k 时的默认值
    sinks: Arc<[Arc<dyn observe::CallSink>]>, // 私有，每次调用扇出到的观测 sink 切片
}
```
下游 MCP server。`Clone` 仅克隆内部 `Arc`（含 `sinks` 切片的 `Arc`），所有克隆共享同一份状态与同一组
sink。

### `GatewayServer::new`
```rust
pub fn new(
    state: Arc<gateway::GatewayState>,
    default_top_k: usize,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
) -> Self
```
用共享网关状态、默认 `top_k`（通常取自 `cfg.retrieval.top_k`）与**观测 sink 切片**构造。`sinks` 为空
切片即「不观测」（每次调用仍会构造记录，只是无人接收）。无错误。

## `fn classify`（私有）
```rust
fn classify(e: &metatools::MetaError) -> (observe::CallOutcome, Option<&'static str>)
```
把 `call_tool` 转发失败的 `metatools::MetaError` 映射为观测用的 `(CallOutcome, error_kind)`：

| `MetaError` 变体 | `CallOutcome` | `error_kind` |
|------------------|---------------|--------------|
| `Timeout` | `Timeout` | `"timeout"` |
| `Call(_)` | `Error` | `"upstream_call"` |
| `ToolNotFound(_)` | `Error` | `"tool_not_found"` |
| `UpstreamUnavailable(_)` | `Error` | `"upstream_unavailable"` |

## `fn meta_tools`
```rust
pub fn meta_tools() -> Vec<rmcp::model::Tool>
```
返回对外暴露的**固定三件套**元工具，顺序为 `["search_tools", "get_tool_details", "call_tool"]`，各带 JSON
object input schema：

- `search_tools`：`query`（string，**required**）+ `top_k`（integer，可选）。
- `get_tool_details`：`name`（string，**required**，qualified name 如 `github__create_issue`）。
- `call_tool`：`name`（string，**required**）+ `arguments`（object，可选）。

与上游数量/变化无关、恒定。私有 `object_schema(json) -> Arc<Map>` 把 `serde_json::json!` 对象包成 rmcp 期望的
`Arc<Map<String, Value>>`（非对象退化为空 map）。无错误。

## `impl ServerHandler for GatewayServer`

### `get_info`
```rust
fn get_info(&self) -> ServerInfo
```
`ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::from_build_env())`。
**只** `enable_tools`，**不** `enable_tool_list_changed`（元工具集合恒定；list_changed 是上游→网关的关注点）。

### `list_tools`
```rust
async fn list_tools(
    &self,
    _request: Option<PaginatedRequestParams>,
    _ctx: RequestContext<RoleServer>,
) -> Result<ListToolsResult, McpError>
```
恒返回 `ListToolsResult::with_all_items(meta_tools())`（3 个元工具），忽略分页参数。永不 `Err`。

### `call_tool`
```rust
async fn call_tool(
    &self,
    request: CallToolRequestParams,
    _ctx: RequestContext<RoleServer>,
) -> Result<CallToolResult, McpError>
```
按 `request.name` 分派（`args = request.arguments.unwrap_or_default()`，每路先 `self.state.snapshot()`）：

- `"search_tools"`：`query`（缺省 `""`）+ `top_k`（缺省 `self.default_top_k`）→ `metatools::search_tools` →
  命中数组 JSON 文本（`CallToolResult::success`）；序列化失败 → `Err(McpError::internal_error)`。
- `"get_tool_details"`：`name`（缺省 `""`）→ `metatools::get_tool_details`；`Some(def)` → JSON 文本，`None` →
  `CallToolResult::error("no such tool: {name}")`（`isError`）。
- `"call_tool"`：缺 `name` → `isError("missing required 'name'")`；否则取可选 `arguments` 对象，
  `metatools::call_tool(&snap, self.state.registry(), name, inner).await`：`Ok` 透传上游 `CallToolResult`、
  `Err(MetaError)` → `CallToolResult::error(e.to_string())`（`isError`）。
- 其它名 → `Err(McpError::invalid_params("unknown tool: {other}"))`（协议级错误）。

**约定**：业务/运行期失败经 `isError` 回传；只有「叫错了元工具名」才返 `McpError`。

### `call_tool` 的调用观测（M6.T1）
每次 `call_tool`（除「未知元工具名」早退分支外）都会构造一条 `observe::CallRecord` 并扇出给 `self.sinks`：

1. 进入即 `let started = Instant::now()`；`arg_bytes = json_len(&args)`（**仅 size**）。`json_len` 用一个仅计数的
   私有 `CountingWriter` + `serde_json::to_writer` 量取序列化 JSON 字节长度，**不分配中间 `String`**（数值与旧的
   `serde_json::to_string(&args).len()` 一致）。
2. 分派 `match` 的每个臂产出五元组 `(response, meta_tool, target_tool, outcome, error_kind)`。`call_tool`
   臂的 `MetaError` 经私有 `classify` 映射；其余 `error_kind` 由内联臂直接给出（见下表）。
3. `match` 结束后**立即** `latency_ms = started.elapsed()`——快照在结果再序列化/`upstream` 派生**之前**，
   故记录的延迟反映调用本身、不含记账开销。
4. `result_bytes = json_len(&response)`（`Err` 路径为 0，**仅 size**；同样经 `CountingWriter` + `to_writer`，无中间 `String`）；
   `upstream = target_tool.split_once("__").map(|(s, _)| s)`（上游 server 前缀）。
5. 构造 `CallRecord { ts_unix_ms: now_unix_ms(), meta_tool, target_tool, upstream, latency_ms, outcome,
   error_kind, arg_bytes, result_bytes }`，`for sink in self.sinks.iter() { sink.record(&rec); }` 同步扇出，
   再返回 `response`。

**`error_kind` 取值表**（`classify` + 内联臂）。注意：上方 `classify` 表的 `CallOutcome` 列是 **Rust 变体名**（`Timeout`/`Error`，即函数返回类型），本表 `outcome` 列是其 **序列化字符串值**（snake_case，如 `timeout`/`error`），两者指同一枚举：

| 触发情形 | `outcome` | `error_kind` |
|----------|-----------|--------------|
| 任一元工具序列化结果失败 → `McpError::internal_error` | `error` | `internal` |
| `get_tool_details` 找不到工具（`None`） | `error` | `tool_not_found` |
| `call_tool` 缺 `name` | `error` | `invalid_params` |
| `call_tool` → `MetaError::Timeout` | `timeout` | `timeout` |
| `call_tool` → `MetaError::Call` | `error` | `upstream_call` |
| `call_tool` → `MetaError::ToolNotFound` | `error` | `tool_not_found` |
| `call_tool` → `MetaError::UpstreamUnavailable` | `error` | `upstream_unavailable` |
| 成功 | `ok` | `None` |
| **未知元工具名**（早退 `McpError::invalid_params`） | — | **不记录**（协议误用，非网关调用） |

**仅元数据不变量**：记录只含上述字段，`arg_bytes`/`result_bytes` 是 size、**无任何参数/结果内容**，故
观测绝不泄露 secret/PII。

> 详见 L3：[downstream](../L3-details/downstream.md)
