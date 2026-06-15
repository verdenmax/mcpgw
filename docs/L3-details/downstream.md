# L3 — `downstream` 细节

## HTTP 鉴权层细节（`http.rs` 的 `require_api_key` 中间件）

`build_router` 在 `api_keys` 非空时叠加一层 axum `from_fn_with_state` 中间件 `require_api_key`：

- **Bearer 提取**：`presented_bearer` 从 `Authorization` 头读字符串，`strip_prefix("Bearer ")` 取出 key；
  缺头/非 ASCII/无 `Bearer ` 前缀都视为「未呈现」。
- **常量时间比较**：`key_authorized` 用 `subtle::ConstantTimeEq::ct_eq` 对每个配置 key 与呈现值逐字节比较，
  把结果按位 `|=` 累积成 `matched`（**不**在命中后提前 `return`，避免泄露「命中了第几个 key」的时序）。
  `ct_eq` 对长度不同的 `&[u8]` 会短路返回 `Choice(0)`——只泄露长度，可接受；长度相同时做常量时间比较。
- **401 不回显期望值**：校验失败一律 `StatusCode::UNAUTHORIZED.into_response()`，**不**在响应里带任何关于
  期望 key 的信息（不区分「缺 key」「错 key」的错误体）。
- **keyset 为空时放行**：不挂这层中间件，依赖 **localhost 绑定 + rmcp `allowed_hosts`**
  （默认 `[localhost, 127.0.0.1, ::1]`）作为唯一防线。

## rmcp `ServerHandler` 三个方法

`GatewayServer` 手写实现 `ServerHandler`（不用 `#[tool_router]` 宏，因为元工具是固定三件套、且 `call_tool`
要自定义分派与错误映射）：

- **`get_info`**：`ServerInfo::new(ServerCapabilities::builder().enable_tools().build())` + 构建环境信息。
  **只** `enable_tools`，**不** `enable_tool_list_changed`——见下文。
- **`list_tools`**：直接 `ListToolsResult::with_all_items(meta_tools())`，忽略分页参数，恒返回 3 个元工具。
- **`call_tool`**：按 `request.name` 分派（下表）。

## `call_tool` 分派表

| 客户端调用名 | 取参 | 委派 | 成功返回 | 失败/缺省 |
|--------------|------|------|----------|-----------|
| `search_tools` | `query: &str`（缺省 `""`）、`top_k: u64`（缺省 `self.default_top_k`） | `metatools::search_tools(&snap, query, top_k)` | 命中数组的 JSON 文本 | 序列化失败 → `McpError::internal_error` |
| `get_tool_details` | `name: &str`（缺省 `""`） | `metatools::get_tool_details(&snap, name)` | 找到 → `ToolDef` JSON 文本 | `None` → `isError`（`"no such tool: {name}"`） |
| `call_tool` | `name: &str`（缺）、`arguments: object`（可选） | `metatools::call_tool(&snap, registry, name, inner).await` | 上游的 `CallToolResult` 透传 | 缺 `name` → `isError`；`MetaError` → `isError` |
| 其它名 | — | — | — | `McpError::invalid_params("unknown tool: {other}")` |

每次分派都先 `let snap = self.state.snapshot()`（**无锁**加载当前快照），`call_tool` 另取
`self.state.registry()` 做上游转发。

## `MetaError` → `isError`，未知名 → `McpError`

两类失败被刻意区别对待：

- **业务/运行期失败**（工具不存在、上游不可用、上游超时、上游报错）来自 `metatools`，被包成
  `CallToolResult::error(...)`（即 MCP 的 `isError: true`）。客户端拿到的是一次**成功的 MCP 调用**、内容标记为
  错误——符合 MCP「工具执行错误经结果体回传」的约定，LLM 可读取并重试。
- **协议级误用**（调用了一个根本不存在的元工具名）返回 `Err(McpError::invalid_params)`，即 JSON-RPC 错误。
  这区分了「这个工具运行失败」与「你叫错了工具」。

## 调用观测（M6.T1，仅元数据）

`call_tool` 在分派之外还**在调用边界构造一条 `observe::CallRecord` 并扇出给 `self.sinks`**——这是 T1
（tracing）与 T3（审计 JSONL）共享的「instrument → multi-sink」埋点。**埋点只在 `downstream`**：
`metatools` crate 保持纯函数、**不**依赖 `observe`。

- **埋点位置**：埋点逻辑全在 `GatewayServer::call_tool` 内。分派 `match` 的每个臂返回五元组
  `(response, meta_tool, target_tool, outcome, error_kind)`；`match` 之后统一构造记录、`for sink in
  self.sinks.iter() { sink.record(&rec); }` 同步扇出，最后返回 `response`。空 `sinks` 即「不观测」。
- **延迟测量基准**：进入即 `let started = Instant::now()`；`match` 一结束**立刻** `latency_ms =
  started.elapsed()`，**早于**结果再序列化（`result_bytes`）与 `upstream` 派生。故 `latency_ms` 反映
  **分派本身**，不含观测记账开销。
- **`arg_bytes` / `result_bytes` 基准**：`arg_bytes = serde_json::to_string(&args).len()`（进入时算一次）；
  `result_bytes = serde_json::to_string(&response).len()`，`Err`（协议错误）路径记 `0`。两者都是
  **字节数（size）**，**绝不含**任何参数/结果内容。
- **`upstream` 派生**：`target_tool.split_once("__").map(|(s, _)| s)` 取 qualified name 的**上游 server 前缀**
  （如 `github__create_issue` → `github`）；只有 `call_tool` 成功/失败带 `target_tool` 时才有值。
- **`outcome` / `error_kind` 分类**：`call_tool` 转发失败经私有 `classify(&MetaError)` 映射，其余由分派臂
  内联给出：

  | 触发情形 | `outcome` | `error_kind` |
  |----------|-----------|--------------|
  | 任一元工具序列化结果失败（`McpError::internal_error`） | `error` | `internal` |
  | `get_tool_details` 找不到工具（`None`） | `error` | `tool_not_found` |
  | `call_tool` 缺 `name` | `error` | `invalid_params` |
  | `MetaError::Timeout` | `timeout` | `timeout` |
  | `MetaError::Call` | `error` | `upstream_call` |
  | `MetaError::ToolNotFound` | `error` | `tool_not_found` |
  | `MetaError::UpstreamUnavailable` | `error` | `upstream_unavailable` |
  | 成功 | `ok` | `None` |

- **仅元数据不变量**：记录的类型本身就装不下载荷——只有上述 size 与分类字段，故观测**绝不泄露
  secret/PII**。`observe` 的单测把序列化 key 集合锁死为恰好这 9 个键。
- **未知元工具名不记录**：调用了一个根本不存在的元工具名时，`call_tool` **早退** `Err(McpError::invalid_params)`
  ——这是**协议误用**、不是一次网关工具调用，因此**不**构造记录、**不**扇出。
- **默认 sink**：`mcpgw serve` 注入 `[observe::TracingSink]`，把每条记录发为结构化
  `tracing::info!(meta_tool, target_tool, upstream, latency_ms, outcome, error_kind, arg_bytes,
  result_bytes, "tool_call")` 事件（走 stderr，与日志同流）。stdio 与 HTTP 两条传输**共享同一切片**。

## 为何 `get_info` 只 `enable_tools`、不 `enable_tool_list_changed`

下游对外暴露的工具列表是**静态的三件套**，永远不变。即便上游 `tools/list_changed` 触发网关重建快照，改变的
只是**被检索/可路由的上游工具**，而非客户端能看到的元工具集合。因此下游**不**声明 `list_changed` 能力、也从不
向客户端发该通知。`list_changed` 是 **上游 → 网关** 的单向关注点（由 `upstream::UpstreamClientHandler` +
`gateway::run_rebuild_worker` 处理），不外溢到 **网关 → 客户端**。

## 端到端测试线束（`tests/`）

`tests/server.rs` + `tests/common/mod.rs` 用 `tokio::io::duplex` 起一对内存管道：服务端跑 `GatewayServer`、
客户端是裸 rmcp `()` client，免起进程即可端到端验证：

- `list_tools_returns_exactly_the_three_metatools`：锁定 3 件套不变量。
- `call_tool_dispatches_all_three_metatools`：挂 `MockUpstream`，验证三条分派路径（search 命中 `mock__echo`、
  details 取定义、call 转发回上游）。
- `call_tool_unknown_meta_name_is_protocol_error`：未知名 → 协议错误。
- `call_tool_routes_missing_upstream_tool_to_iserror`：`MetaError::ToolNotFound` → `isError`。
- `list_changed_refreshes_what_search_can_find`：挂 `RevealingMockUpstream` + 真实 `run_rebuild_worker`，经网关
  调用 `reveal` → 上游发 `tools/list_changed` → handler → trigger → worker 重建 → 轮询直到 `search_tools`
  能搜到新冒出的 `mock__late_tool`。这条覆盖了 reveal → notify → rebuild → search 的完整闭环。
- `meta_tool_calls_are_observed_with_metadata`：注入 `observe::CaptureSink`（经
  `connect_to_gateway_with_sinks`），跑 `search_tools` + `call_tool`（命中 `mock__echo`）+ `call_tool`
  （`mock__nope` → `tool_not_found`）+ 未知名，断言**恰好 3 条**记录（未知名不记录）、`target_tool`/`upstream`
  正确派生、成功/失败的 `outcome`/`error_kind` 正确、`arg_bytes`/`result_bytes` 为正。

> 测试线束的默认 sink 为空（`common::no_sinks()`），仅观测专项用例显式注入 `CaptureSink`。

## 相关

- 接口见 L2：[downstream](../L2-components/downstream.md) · [observe](../L2-components/observe.md)
- 逐文件 API 见 L4：[lib](../L4-api/downstream-lib.md) · [observe-lib](../L4-api/observe-lib.md)
- 元工具与错误见：[metatools L3](./metatools.md) · 重建/触发见：[gateway L3](./gateway.md) ·
  上游 list_changed 见：[upstream L3](./upstream.md)
