# L4 — `crates/upstream/src/connection.rs` API

源文件：`crates/upstream/src/connection.rs`。一条到单个上游 MCP 服务器的活连接，外加 list_changed 转发用的
client handler 与触发器类型。

## `type RebuildTrigger`
```rust
pub type RebuildTrigger = tokio::sync::mpsc::Sender<String>;
```
网关用来排空、据以重建快照的有界 channel 的发送端。handler 在每次 `tools/list_changed` 时把上游名 `try_send`
进去；channel 满也无妨（worker 会合并/coalesce 同一波触发）。

## `struct UpstreamClientHandler`
```rust
#[derive(Clone)]
pub struct UpstreamClientHandler { /* server: String, trigger: Option<RebuildTrigger>（私有） */ }
```
装在每条上游连接上的 rmcp `ClientHandler`。`on_tool_list_changed` 时，若 `trigger` 为 `Some` 则
`tx.try_send(self.server.clone())`（忽略发送错误）；`trigger: None` 时为 no-op（内存测试用）。

## `struct UpstreamHandle`
```rust
pub struct UpstreamHandle { /* server: String, client: RunningService<RoleClient, UpstreamClientHandler>, call_timeout: Duration（私有） */ }
```
命名空间名 + 运行中的 rmcp client（带 `UpstreamClientHandler`）+ 每次调用的超时。

### `UpstreamHandle::connect`
```rust
pub async fn connect<T, E, A>(server: &str, transport: T) -> Result<Self, UpstreamError>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
```
在任意 rmcp `IntoTransport`（真实 stdio 子进程或内存 duplex）上建连，**无 list_changed trigger**（内存测试用）。
等价于 `connect_with_trigger(server, transport, None)`。失败返回 `UpstreamError::Connect { server, source }`。
泛型 `IntoTransport` 签名（取代早期裸 `AsyncRead + AsyncWrite` 约束）让同一路径既吃 duplex、又吃 rmcp
`TokioChildProcess`。

### `UpstreamHandle::connect_with_trigger`
```rust
pub async fn connect_with_trigger<T, E, A>(
    server: &str,
    transport: T,
    trigger: Option<RebuildTrigger>,
) -> Result<Self, UpstreamError>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
```
建连并装上携带 `trigger` 的 `UpstreamClientHandler`：`handler.serve(transport).await` 握手。`call_timeout`
初始化为默认 **30s**。失败返回 `UpstreamError::Connect { server, source }`。生产路径由
`connect::connect_stdio_upstream` 调用并传入真实 trigger。

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

### `UpstreamHandle::call_timeout`
```rust
pub fn call_timeout(&self) -> std::time::Duration
```
返回该 handle 配置的每调用超时。网关用它给每个 `ingest_into` 加界（并发 ingest 的 per-ingest 超时）。无错误。

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
消费 `self`，`client.cancel().await` 取消底层 rmcp 服务并 await 其清理（drain + 关闭传输；忽略取消结果）。用于
**独占**（拥有并可 await）拆卸路径。

### `UpstreamHandle::cancel`
```rust
pub fn cancel(&self)
```
经底层 rmcp 服务的 `cancellation_token().cancel()` 取消服务，**不消费 handle**（`&self`）。与 `shutdown(self)`
不同，这是 **fire-and-forget**：只发出取消信号即**立即返回**，不 await 清理。它能作用于一份**共享** `&self`
（如重建 worker 或在途调用仍持有的 `Arc<UpstreamHandle>` clone），故拆卸时**永不静默跳过取消**。触发 token 会停掉
service loop，从而关闭传输并（对子进程上游）回收子进程——与最后一份 `Arc` clone 何时 drop 无关。`shutdown(self)` 仍
保留，用于拥有唯一引用、可 await 完整优雅取消的路径。

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

> 另见 L4：[connect](./upstream-connect.md)（eager-connect / 降级启动 / env allow-list / 握手超时）。
> 详见 L3：[upstream](../L3-details/upstream.md)
