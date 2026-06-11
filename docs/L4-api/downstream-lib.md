# L4 — `crates/downstream/src/lib.rs` API

源文件：`crates/downstream/src/lib.rs`。把网关的 3 个固定元工具经 rmcp `ServerHandler`（stdio）暴露给 MCP 客户端。

## `struct GatewayServer`
```rust
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<gateway::GatewayState>,   // 私有，共享网关状态
    default_top_k: usize,                // 私有，search_tools 省略 top_k 时的默认值
}
```
下游 MCP server。`Clone` 仅克隆内部 `Arc`，所有克隆共享同一份状态。

### `GatewayServer::new`
```rust
pub fn new(state: Arc<gateway::GatewayState>, default_top_k: usize) -> Self
```
用共享网关状态与默认 `top_k`（通常取自 `cfg.retrieval.top_k`）构造。无错误。

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

> 详见 L3：[downstream](../L3-details/downstream.md)
