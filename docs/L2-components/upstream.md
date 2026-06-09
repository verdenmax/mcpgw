# L2 — `upstream` 组件

## 职责

活的**上游 MCP I/O 层**（M1-A）：用 rmcp client 连接上游 MCP 服务器、把上游工具**摄取**成命名空间化的
`catalog::ToolDef`、并把 `call_tool` 调用转发回对应上游。维护一份按 server 名键入的连接注册表。不了解检索、
配置或 CLI；不暴露 MCP 元工具（那是 M1-B 网关的事）。

## 公开接口

### 类型 `UpstreamHandle`（`connection.rs`）
一条到单个上游 MCP 服务器的活连接：命名空间名 + 运行中的 rmcp client。

| 方法 | 签名 | 说明 |
|------|------|------|
| `connect` | `async (server: &str, transport: T) -> Result<Self, UpstreamError>`，`T: AsyncRead+AsyncWrite+Send+Unpin+'static` | 在任意 async-rw 传输上握手建连（真实 stdio 子进程或内存 duplex） |
| `server` | `(&self) -> &str` | 该连接的命名空间名 |
| `ingest_into` | `async (&self, &mut Catalog) -> Result<usize, UpstreamError>` | 拉取该 server 工具，命名空间化后摄取进 catalog；返回跳过的重复数 |
| `call_tool` | `async (&self, tool: &str, args: Option<serde_json::Map<String, Value>>) -> Result<CallToolResult, UpstreamError>` | 转发调用；`tool` 是**原始**（未命名空间化）名 |
| `shutdown` | `async (self)` | 取消底层 rmcp 服务 |

### 错误 `UpstreamError`（`connection.rs`）
`#[derive(thiserror::Error)]` 枚举，两变体均带 `server: String` 与 boxed `source`：

- `Connect { server, source }` — 建连失败。
- `Call { server, source }` — `list_all_tools` / `call_tool` 失败。

### 类型 `UpstreamRegistry`（`registry.rs`）
线程安全注册表，`server name -> Arc<UpstreamHandle>`。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `() -> Self` | 空注册表（= `Default`） |
| `insert` | `(&self, Arc<UpstreamHandle>)` | 按 `handle.server()` 插入/替换 |
| `get` | `(&self, &str) -> Option<Arc<UpstreamHandle>>` | 按名取一份 `Arc` 克隆 |
| `remove` | `(&self, &str) -> Option<Arc<UpstreamHandle>>` | 摘除并返回 `Arc`（可用于 graceful shutdown） |
| `server_names` | `(&self) -> Vec<String>` | 已注册 server 名，**升序排序** |

### 枚举 `UpstreamState`（`registry.rs`）
`Connecting` / `Ready` / `Failed`，连接生命周期状态。目前仅为枚举定义，由 M1-B 接入。

### 映射 `mapping::{tool_to_def, ingest_tools}`（`mapping.rs`）

| 函数 | 签名 | 说明 |
|------|------|------|
| `tool_to_def` | `(server: &str, tool: &rmcp::model::Tool) -> ToolDef` | 单工具 → 命名空间化 `ToolDef` |
| `ingest_tools` | `(catalog: &mut Catalog, server: &str, tools: &[Tool]) -> usize` | 批量摄取（intra-server first-dupe-wins，warn），返回被跳过的重复名计数 |

### 测试件 `testkit::MockUpstream`（`testkit.rs`，`testkit` feature）
内存 mock MCP 服务器，暴露 `echo`、`greet` 两个工具。供本 crate 单测与集成测试使用，并可经 `testkit`
feature 被其它 crate 复用。

## 依赖

- 外部：`rmcp`（1.7，client/server/macros/transport）、`tokio`、`thiserror`、`tracing`、`serde_json`、`schemars`。
- 内部：`catalog`（摄取目标类型 `ToolDef` / `Catalog`）。

## 被谁使用

- `gateway`（M1-B）：装配 `UpstreamRegistry`，把上游工具摄取进 catalog，并经元工具 `call_tool` 路由到对应
  `UpstreamHandle`。

## 关键不变量

- 命名空间恒为 `{server}__{name}`（沿用 `catalog` 命名空间方案）。
- 单次摄取内 **intra-server first-dupe-wins**：同名工具保留首个、其余 warn+skip。
- 注册表成员仅在显式 `insert`/`remove`（连接建立/断开）时变化；`get`/`server_names` 不改变成员。

## 向下导航

- 内部细节见 L3：[upstream](../L3-details/upstream.md)
- 逐文件 API 见 L4：[mapping](../L4-api/upstream-mapping.md) · [connection](../L4-api/upstream-connection.md) ·
  [registry](../L4-api/upstream-registry.md)
