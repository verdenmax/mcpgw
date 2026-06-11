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

## 相关

- 接口见 L2：[downstream](../L2-components/downstream.md)
- 逐文件 API 见 L4：[lib](../L4-api/downstream-lib.md)
- 元工具与错误见：[metatools L3](./metatools.md) · 重建/触发见：[gateway L3](./gateway.md) ·
  上游 list_changed 见：[upstream L3](./upstream.md)
