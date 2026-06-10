# L4 — `crates/metatools/src/tools.rs` API

源文件：`crates/metatools/src/tools.rs`。`GatewaySnapshot` 之上的三个元工具函数。

## `search_tools`
```rust
pub fn search_tools(snap: &GatewaySnapshot, query: &str, top_k: usize) -> Vec<ToolSummary>
```
经 `snap.strategy.search(query, top_k)` 检索，把每个 `retrieval::ScoredTool` 投影为
`ToolSummary { name: hit.qualified_name, description: hit.description }`（`score` 丢弃），保持"最佳在前"。
无错误：无命中返回空 `Vec`。

## `get_tool_details`
```rust
pub fn get_tool_details<'a>(snap: &'a GatewaySnapshot, name: &str) -> Option<&'a ToolDef>
```
`snap.catalog.get(name)`：按命名空间名（`{server}__{name}`）返回对快照内 `catalog::ToolDef` 的借用，
生命周期绑定 `&snap`。无命中返回 `None`。

## `call_tool`
```rust
pub async fn call_tool(
    snap: &GatewaySnapshot,
    registry: &UpstreamRegistry,
    name: &str,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<rmcp::model::CallToolResult, MetaError>
```
路由一次工具调用：

1. `snap.catalog.get(name)` 取 `ToolDef`；无则 `Err(MetaError::ToolNotFound(name))`。
2. `registry.get(&def.server)` 取上游 handle；无则 `Err(MetaError::UpstreamUnavailable(def.server))`。
3. `handle.call_tool(&def.name, arguments).await`，用 catalog 中存好的**原始**工具名 `def.name`
   （**不**对命名空间名 split `"__"`）。

`UpstreamError` 映射：`Timeout { .. }` → `MetaError::Timeout`；其余 → `MetaError::Call(e.to_string())`。

## `enum MetaError`（`error.rs`）
```rust
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("no such tool: {0}")]
    ToolNotFound(String),
    #[error("upstream {0:?} is unavailable")]
    UpstreamUnavailable(String),
    #[error("upstream call timed out")]
    Timeout,
    #[error("upstream call failed: {0}")]
    Call(String),
}
```
元工具函数的错误类型，由下游服务（M1-B.2）映射为 MCP `isError`：

- `ToolNotFound(name)` — catalog 查不到该命名空间名。
- `UpstreamUnavailable(server)` — 该工具所属 server 不在注册表中。
- `Timeout` — 上游调用超时（自 `UpstreamError::Timeout` 转来，省去 server 信息）。
- `Call(msg)` — 其它上游调用失败，携带底层 `UpstreamError` 文本。

> 详见 L3：[metatools](../L3-details/metatools.md)
