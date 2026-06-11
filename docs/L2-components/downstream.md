# L2 — `downstream` 组件

## 职责

网关的**下游 MCP 服务层**（M1-B.2）：用 rmcp `ServerHandler` 把网关的三个固定**元工具**
（`search_tools` / `get_tool_details` / `call_tool`）作为 MCP 工具暴露给客户端（经 stdio）。它持有一份共享
`gateway::GatewayState`，把每次 `call_tool` 分派给 `metatools` 的对应纯函数。它**不**持有可变状态、**不**做
eager-connect 或起 worker（那是 `mcpgw serve` 的事），也**不**直接连接上游（路由经 `GatewayState` 的注册表）。

## 公开接口

### 类型 `GatewayServer`（`lib.rs`）
下游 MCP server。持有共享网关状态 + 一个 `default_top_k`（`search_tools` 省略 `top_k` 时使用，来自
`[retrieval].top_k`）。`#[derive(Clone)]`（仅克隆内部 `Arc`）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(state: Arc<GatewayState>, default_top_k: usize) -> Self` | 用共享状态与默认 `top_k` 构造 |

实现 `rmcp::ServerHandler`：`get_info`（仅 `enable_tools`）、`list_tools`（恒返回 3 个元工具）、`call_tool`
（按名分派到 `metatools`）。配合 `server.serve(stdio()).await` 起服务。

### 函数 `meta_tools`（`lib.rs`）

| 函数 | 签名 | 说明 |
|------|------|------|
| `meta_tools` | `() -> Vec<rmcp::model::Tool>` | 对外暴露的**固定三件套**元工具（含各自 JSON input schema）；与上游无关、恒定 |

### HTTP server（Streamable HTTP）职责（`http.rs`）

除 stdio 外，下游还可经 **Streamable HTTP** transport 暴露**同一个** `GatewayServer`（3 个元工具完全一致，
只是换了 transport）。`http::build_router` 用 rmcp `StreamableHttpService` 把服务装进 axum `Router`：

| 函数 | 签名 | 说明 |
|------|------|------|
| `build_router` | `(state: Arc<GatewayState>, default_top_k: usize, path: &str, api_keys: Vec<String>) -> axum::Router` | 把 3 个元工具挂在 `path`（如 `/mcp`）下，返回可供 `axum::serve` 起监听的 `Router` |

- 工厂闭包 `move || Ok(GatewayServer::new(state.clone(), default_top_k))` 为每个会话复用同一份共享状态。
- `StreamableHttpService` 实现 `tower_service::Service`，直接 `Router::new().nest_service(path, service)` 挂载。
- `StreamableHttpServerConfig::default()` 的 `allowed_hosts` 默认 `[localhost, 127.0.0.1, ::1]`，放行本机。
- `api_keys` **现仅为稳定签名而接受、暂不使用**；Bearer 鉴权在 M1-C T4 叠加。

## 依赖

- 内部：`gateway`（`GatewayState`：共享快照 + 上游注册表）、`metatools`（三个元工具函数 + `MetaError`）。
- 外部：`rmcp`（`ServerHandler` / `Tool` / `CallToolResult` 等，feature `server` + `transport-io` +
  `transport-streamable-http-server`）、`axum`（0.8，HTTP server router）、`serde_json`。

> 注：`call_tool` 序列化 `metatools` 返回的 `ToolDef`/`ToolSummary`，但不直接命名 `catalog` 类型，
> 故无需直接依赖 `catalog`/`tracing`/`tokio`（这些已从 `[dependencies]` 移除；`tokio` 仅在 dev 测试中用）。

## 被谁使用

- `mcpgw`（bin）的 `serve` 子命令：`GatewayServer::new(state, top_k).serve(stdio())` 起下游服务，连同
  `connect_all` 与 `run_rebuild_worker` 一起组成活网关。

## 关键不变量

- **`list_tools` 恒等于这 3 个元工具**：与上游数量/变化无关；元工具集合是常量（故 `get_info` **不**声明
  `list_changed`——快照变化对客户端可见的工具集没有影响）。
- **错误映射分层**：`metatools::MetaError`（如工具不存在、上游不可用、超时）→ `CallToolResult` 的 `isError`
  返回给客户端；**未知元工具名** → MCP 协议层错误（`McpError::invalid_params`）。
- 自身**无可变状态**：所有数据经 `GatewayState::snapshot()`（读无锁）与 `registry()` 取得。

## 向下导航

- 内部细节见 L3：[downstream](../L3-details/downstream.md)
- 逐文件 API 见 L4：[lib](../L4-api/downstream-lib.md) · [http](../L4-api/downstream-http.md)
- 元工具逻辑见：[metatools L2](./metatools.md) · 状态/重建见：[gateway L2](./gateway.md)
