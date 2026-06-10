# L4 — `crates/upstream/src/connection.rs` API

源文件：`crates/upstream/src/connection.rs`。一条到单个上游 MCP 服务器的活连接。

## `struct UpstreamHandle`
```rust
pub struct UpstreamHandle { /* server: String, client: RunningService<RoleClient, ()>, call_timeout: Duration（私有） */ }
```
命名空间名 + 运行中的 rmcp client + 每次调用的超时。

### `UpstreamHandle::connect`
```rust
pub async fn connect<T>(server: &str, transport: T) -> Result<Self, UpstreamError>
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
```
在任意 async-rw 传输（真实 stdio 子进程或内存 duplex）上 `().serve(transport)` 握手建连。`call_timeout` 初始化为
默认 **30s**。失败返回 `UpstreamError::Connect { server, source }`。

### `UpstreamHandle::with_call_timeout`
```rust
pub fn with_call_timeout(mut self, timeout: std::time::Duration) -> Self
```
设置每次 `call_tool` 的超时并返回 `self`（builder 风格，在 `Arc` 共享前消费）。无错误。

### `UpstreamHandle::server`
```rust
pub fn server(&self) -> &str
```
返回该连接的命名空间名。无错误。

### `UpstreamHandle::ingest_into`
```rust
pub async fn ingest_into(&self, catalog: &mut catalog::Catalog) -> Result<usize, UpstreamError>
```
`list_all_tools()` 拉取该 server 工具，再 `ingest_tools` 命名空间化摄取进 `catalog`。列工具失败返回
`UpstreamError::Call`。成功时返回被跳过的 intra-server 重复工具名数量（也会 warn），供调用方上报摄取统计。

### `UpstreamHandle::call_tool`
```rust
pub async fn call_tool(
    &self,
    tool: &str,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<rmcp::model::CallToolResult, UpstreamError>
```
转发一次工具调用。`tool` 是**原始（未命名空间化）**名。`arguments` 为 `None` 时不带参。构造
`CallToolRequestParams::new(tool)`（有参则 `.with_arguments(args)`）并以 `tokio::time::timeout(self.call_timeout, …)`
施加**每次调用超时**转发。超时返回 `UpstreamError::Timeout { server }`；其它失败返回 `UpstreamError::Call`。

### `UpstreamHandle::shutdown`
```rust
pub async fn shutdown(self)
```
消费 `self`，`client.cancel().await` 取消底层 rmcp 服务（忽略取消结果）。

## `enum UpstreamError`
```rust
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("failed to connect to upstream {server:?}: {source}")]
    Connect {
        server: String,
        #[source] source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("upstream {server:?} call failed: {source}")]
    Call {
        server: String,
        #[source] source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("upstream {server:?} call timed out")]
    Timeout { server: String },
}
```
- `Connect`：`connect` 建连失败（`source` 为 rmcp 建连错误）。
- `Call`：`ingest_into` 的 `list_all_tools` 或 `call_tool` 失败。
- `Timeout`：`call_tool` 超过 `call_timeout` 未应答（仅带 `server`，无 `source`）。
- `Connect`/`Call` 都带 `server` 命名空间名，`source` 装箱以解耦 rmcp 具体错误类型。

> 详见 L3：[upstream](../L3-details/upstream.md)
