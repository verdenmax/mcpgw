# L4 — `crates/downstream/src/lib.rs` API

源文件：`crates/downstream/src/lib.rs`。把网关的 3 个固定元工具经 rmcp `ServerHandler`（stdio）暴露给 MCP 客户端，并在每次调用边界构造一条 `observe::CallRecord` 扇出到注入的 sink（**仅元数据**）；随后若 `content_sinks` 非空，再把一条**截断后的** `observe::CallContent`（args/result 文本）扇出到 `content_sinks`（**与元数据通道物理隔离**，只入 dashboard 内存环）。

## `struct GatewayServer`
```rust
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<gateway::GatewayState>,             // 私有，共享网关状态
    default_top_k: usize,                          // 私有，search_tools 省略 top_k 时的默认值
    sinks: Arc<[Arc<dyn observe::CallSink>]>,      // 私有，每次调用扇出到的观测 sink 切片
    discovery: Arc<[Arc<dyn observe::DiscoverySink>]>, // 私有，search_tools 扇出的发现追踪 sink 切片
    content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>, // 私有，call_tool 扇出调用内容的 sink 切片
    payload_max_bytes: usize,                      // 私有，单条 args/result 捕获的字节上限
}
```
下游 MCP server。`Clone` 仅克隆内部 `Arc`（含 `sinks`/`discovery`/`content_sinks` 切片的 `Arc`），所有克隆共享
同一份状态、同一组观测 sink、同一组发现追踪 sink 与同一组内容 sink。

### `GatewayServer::new`
```rust
pub fn new(
    state: Arc<gateway::GatewayState>,
    default_top_k: usize,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
    discovery: Arc<[Arc<dyn observe::DiscoverySink>]>,
    content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>,
    payload_max_bytes: usize,
) -> Self
```
用共享网关状态、默认 `top_k`（通常取自 `cfg.retrieval.top_k`）、**观测 sink 切片**、**发现追踪 sink 切片**、
**内容 sink 切片**与**载荷字节上限**构造。`sinks` 为空切片即「不观测」（每次调用仍会构造记录，只是无人接收）；
`discovery` 为**空切片即不捕获发现追踪**（`search_tools` 跳过构造 `DiscoveryRecord`）；`content_sinks` 为**空切片
即不捕获调用内容**（`call_tool` 跳过 `CallContent` 的构造与扇出，省去 args/result 的序列化+截断开销）。
`payload_max_bytes` 是单条 `args`/`result` 文本各自的字节上限（装配取自 `[dashboard].payload_max_bytes`）。无错误。

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

## `fn discovery_record_for_search`（私有）
```rust
fn discovery_record_for_search(
    query: &str,
    top_k: usize,
    hits: &[metatools::ToolSummary],
    latency_ms: u64,
) -> observe::DiscoveryRecord
```
从一次完成的 `search_tools` 调用构造一条**发现追踪**（纯函数）：`ts_unix_ms = now_unix_ms()`、经 `clamp_query`
截到 `MAX_TRACE_QUERY_CHARS = 2048` 字符的 `query`、`top_k`、把每个 `ToolSummary` 映成 `DiscoveryHit { name, score }`
（即 `ToolSummary.score` 的去处）、`latency_ms`。仅当 `self.discovery` 非空时调用——`search_tools` 臂随后
`for sink in self.discovery.iter() { sink.record(&drec) }` 扇出（见下）。

**查询长度封顶（N2）**：`const MAX_TRACE_QUERY_CHARS = 2048` + 私有 `clamp_query(&str) -> String`
（`query.chars().take(2048).collect()`，按 `char` 截断、**UTF-8 安全、绝不切碎码点**）把捕获的 client query 截短。
discovery ring 已按 `trace_buffer` 封顶条数，但此前存的是**逐字 client query**，故海量超长 query（开了
`trace_queries` 时）会放大内存；截断后 `DiscoveryRingSink` 的常驻内存按 `trace_buffer × 2048 字符`有界（不随
client 输入大小膨胀）。短查询原样保留。

## 调用内容截断辅助（私有）
```rust
fn truncate_utf8(s: String, cap: usize) -> (String, bool)
fn cap_json<T: serde::Serialize>(value: &T, cap: usize) -> (String, bool)
fn cap_response(response: &Result<CallToolResult, McpError>, cap: usize) -> (String, bool)
```
为 `call_tool` 的内容捕获服务的三个**UTF-8 安全**辅助：

- `truncate_utf8`：把 `s` 截到**至多** `cap` 字节，返回 `(可能被截断的串, 是否截断)`。`s.len() <= cap` 时原样
  返回（`truncated = false`）；否则从 `cap` 处向左回退到最近的 `char` 边界（`is_char_boundary`），**绝不切碎
  码点**。
- `cap_json`：先 `serde_json::to_string(value)` 紧凑序列化，再 `truncate_utf8` 截到 `cap`；序列化失败 →
  `("<unserializable>", false)`。用于 `args`。
- `cap_response`：把调用响应映成文本——`Ok(r)` 走 `cap_json(r, cap)`（序列化结果），`Err(e)` 走
  `truncate_utf8(e.to_string(), cap)`（**上游错误纯文本**）。用于 `result`，故失败调用的错误文本也被捕获。

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
  命中数组 JSON 文本（`CallToolResult::success`）；序列化失败 → `Err(McpError::internal_error)`。**发现追踪**：
  在序列化前，若 `self.discovery` 非空，用 `discovery_record_for_search(query, top_k, &hits,
  started.elapsed())` 构造一条 `DiscoveryRecord` 并 `for sink in self.discovery.iter() { sink.record(&drec) }`
  扇出（空 catalog → 空 `results`，仍追踪）。
- `"get_tool_details"`：`name`（缺省 `""`）→ `metatools::get_tool_details`；`Some(def)` → JSON 文本，`None` →
  `CallToolResult::error("no such tool: {name}")`（`isError`）。
- `"call_tool"`：缺 `name` → `isError("missing required 'name'")`；否则取可选 `arguments` 对象，
  `metatools::call_tool(&snap, self.state.registry(), name, inner).await`：`Ok` 透传上游 `CallToolResult`
  （结果原样转发；若其 `is_error == Some(true)` 则观测判为 `Error` / `upstream_tool_error`，见下「`error_kind` 取值表」）、
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
   `upstream = target_tool` 经**工具目录解析**取真实 server：`get_tool_details(&self.state.snapshot(),
   t).map(|def| def.server.clone())`（**安全修复**：不再 `split_once("__")` 切 client 提供的名字，故未知
   `call_tool` 名解析不到时 `upstream = None`，不会注入 attacker 可控前缀 — 见下「`upstream` 归因」）。
5. 构造 `CallRecord { ts_unix_ms: now_unix_ms(), meta_tool, target_tool, upstream, latency_ms, outcome,
   error_kind, arg_bytes, result_bytes }`，`for sink in self.sinks.iter() { sink.record(&rec); }` 同步扇出，
   再返回 `response`。
6. **调用内容捕获（M1）**：在上述**仅元数据扇出之后**，若 `self.content_sinks` **非空**（空则整段跳过），用
   `cap_json(&args, self.payload_max_bytes)` 与 `cap_response(&response, self.payload_max_bytes)` 各取
   `(文本, 是否截断)`，构造 `observe::CallContent { args, args_truncated, result, result_truncated }`，再
   `for s in self.content_sinks.iter() { s.record(&rec, &content); }` 扇出（**同一条** `rec` 同时给内容 sink，
   省去重复元数据）。内容**只**入 `content_sinks`（dashboard 内存环），**绝不**进 `sinks`，故 args/result 永不
   漏进 tracing/审计；失败调用的上游错误文本经 `cap_response` 的 `Err` 臂一并捕获。

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
| `call_tool` 成功往返但上游结果 `is_error=true`（结果原样转发，仅观测判 `error`） | `error` | `upstream_tool_error` |
| 成功 | `ok` | `None` |
| **未知元工具名**（早退 `McpError::invalid_params`） | — | **不记录**（协议误用，非网关调用） |

**仅元数据不变量**：记录只含上述字段，`arg_bytes`/`result_bytes` 是 size、**无任何参数/结果内容**，故
观测绝不泄露 secret/PII。

**`upstream` 归因（安全修复）**：`upstream` 由对 `target_tool` 的**工具目录解析**得到真实 `def.server`，
**不**靠拆分 client 提供的 `call_tool` 名。否则一个未知/构造的 `call_tool` 名（`ToolNotFound`）会让
`split_once("__")` 切出一个**无界、attacker 可控**的 `upstream` 前缀，污染指标并能灌爆 dashboard 的
`per_upstream` 维度；修复后这类调用 `upstream = None`。

**发现追踪（dashboard）**：与上述仅元数据扇出**相互独立**——`search_tools` 在 `self.discovery` 非空时另扇出
一条 `observe::DiscoveryRecord`（含 query 文本 + 命中工具名/分数）。它走 `DiscoverySink`，**绝不**进 `sinks`
（仅元数据通道），故 query 永不漏进 tracing/审计。

**调用内容捕获（dashboard）**：同样与仅元数据扇出**相互独立**——`call_tool` 在 `self.content_sinks` 非空时另扇出
一条 `observe::CallContent`（截断后的 args/result 文本，含上游错误文本）。它走 `CallContentSink`，**绝不**进
`sinks`，故内容永不漏进 tracing/审计；内容只入 dashboard 内存环，按 `payload_max_bytes` 单条 UTF-8 截断、
重启即丢。`build_router`（HTTP 传输）也新增 `content_sinks` / `payload_max_bytes` 两参并透传给每会话的
`GatewayServer`（见 [downstream-http](./downstream-http.md)）。

> 详见 L3：[downstream](../L3-details/downstream.md)
