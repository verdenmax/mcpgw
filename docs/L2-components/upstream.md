# L2 — `upstream` 组件

## 职责

活的**上游 MCP I/O 层**（M1-A）：用 rmcp client 连接上游 MCP 服务器（**stdio 子进程或远程 HTTP**）、把上游工具
**摄取**成命名空间化的 `catalog::ToolDef`、并把 `call_tool` 调用转发回对应上游。维护一份按 server 名键入的连接
注册表。不了解检索、配置或 CLI；不暴露 MCP 元工具（那是 M1-B 网关的事）。

## 公开接口

### 类型 `UpstreamHandle`（`connection.rs`）
一条到单个上游 MCP 服务器的活连接：命名空间名 + 运行中的 rmcp client。

| 方法 | 签名 | 说明 |
|------|------|------|
| `connect` | `async <T,E,A>(server: &str, transport: T) -> Result<Self, UpstreamError>`，`T: IntoTransport<RoleClient,E,A>` | 在任意 rmcp `IntoTransport`（真实 stdio 子进程或内存 duplex）上握手建连，**无 list_changed trigger**；`call_timeout` 默认 30s |
| `connect_with_trigger` | `async <T,E,A>(server: &str, transport: T, trigger: Option<RebuildTrigger>) -> Result<Self, UpstreamError>` | 同上，但装上携带 `trigger` 的 `UpstreamClientHandler`（上游 `tools/list_changed` → 推动网关重建） |
| `with_call_timeout` | `(self, timeout: Duration) -> Self` | builder：设定每次 `call_tool` 超时（`Arc` 共享前消费） |
| `server` | `(&self) -> &str` | 该连接的命名空间名 |
| `call_timeout` | `(&self) -> Duration` | 该 handle 的每调用超时（网关用它给每个并发 `ingest_into` 加界） |
| `ingest_into` | `async (&self, &mut Catalog) -> Result<usize, UpstreamError>` | 拉取该 server 工具，命名空间化后摄取进 catalog；返回跳过的重复数 |
| `call_tool` | `async (&self, tool: &str, args: Option<serde_json::Map<String, Value>>) -> Result<CallToolResult, UpstreamError>` | 转发调用（带 `call_timeout` 超时）；`tool` 是**原始**（未命名空间化）名 |
| `shutdown` | `async (self)` | 取消底层 rmcp 服务 |

### list_changed 转发：`RebuildTrigger` / `UpstreamClientHandler`（`connection.rs`）
- `RebuildTrigger = tokio::sync::mpsc::Sender<String>`：网关排空、据以重建快照的有界 channel 发送端。
- `UpstreamClientHandler`：装在每条连接上的 rmcp `ClientHandler`；`on_tool_list_changed` 时把上游名 `try_send`
  进 trigger（`trigger: None` 时为 no-op，内存测试用）。channel 满也无妨——worker 会合并同一波触发。

### eager-connect：`connect_all` / `connect_stdio_upstream` / `connect_http_upstream` / `ConnectSummary`（`connect.rs`）
按配置 eager-connect 所有上游（**按 `transport` 分派** stdio 子进程或远程 HTTP），**降级启动**：单上游连不上只被
记录、不阻断其余。

| 项 | 签名 | 说明 |
|----|------|------|
| `connect_all` | `async (&UpstreamRegistry, &[UpstreamConfig], RebuildTrigger) -> ConnectSummary` | 逐个连接，按 `transport` 分派 stdio/http：成功 `insert` 进注册表并记入 `connected`，失败 `warn!`+记入 `skipped`（不 `Err`） |
| `connect_stdio_upstream` | `async (&UpstreamConfig, Option<RebuildTrigger>) -> Result<UpstreamHandle, UpstreamError>` | spawn 子进程并连接，**握手受 `call_timeout_ms` 超时约束**，并施加 env allow-list |
| `connect_http_upstream` | `async (&UpstreamConfig, Option<RebuildTrigger>) -> Result<UpstreamHandle, UpstreamError>` | 连接远程 HTTP MCP server：从 env 解析 `bearer_env`（**原始 token**，rmcp 在线路上自动加 `Bearer ` 前缀）与 `headers`（头名→env），构造 `StreamableHttpClientTransport` 后**复用泛型 `connect_with_trigger`**（同一握手超时 + per-call 超时 + list_changed 管线） |
| `ConnectSummary` | `{ connected: Vec<String>, skipped: Vec<(String, String)> }` | 哪些上游连上 / 跳过（含原因） |

**http transport 复用泛型连接路径**：HTTP 上游不另起一条管线——`connect_http_upstream` 仅负责把 env 引用的 auth
组装成 rmcp `StreamableHttpClientTransportConfig`，随后交给与 stdio **同一个** `UpstreamHandle::connect_with_trigger`
（这是 M1-B.2 把签名泛化为 `IntoTransport<RoleClient, E, A>` 的收益）。缺 env / 非法头 / 网络不可达均映射为
`UpstreamError`，在 `connect_all` 里同样降级隔离。

### 错误 `UpstreamError`（`connection.rs`）
`#[derive(thiserror::Error)]` 枚举：

- `Connect { server, source }` — 建连失败。
- `Call { server, source }` — `list_all_tools` / `call_tool` 失败（均带 `server: String` 与 boxed `source`）。
- `Timeout { server }` — `call_tool` 超过 `UpstreamHandle` 的 `call_timeout` 未应答。

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

### 测试件 `testkit`（`testkit.rs`，`testkit` feature）
- `MockUpstream`：内存 mock MCP 服务器，暴露**固定** `echo`、`greet`、`slow` 三个工具（`slow` 故意 sleep，
  用于触发 per-call 超时）。
- `RevealingMockUpstream`：**运行期变更工具列表**的 mock——初始暴露 `echo` + `reveal`，调用 `reveal` 后才冒出
  `late_tool` 并向客户端发 `tools/list_changed`，用于端到端驱动网关的 list_changed 重建路径。
- testkit-only 二进制 `mock-stdio`（`src/bin/mock-stdio.rs`，`required-features = ["testkit"]`）：把
  `MockUpstream` 跑在真实 stdio 上，供冒烟验证子进程 connect 路径（`connect_stdio_upstream`）对接真实子进程。

供本 crate 单测/集成测试使用，并可经 `testkit` feature 被其它 crate（如 `downstream` 的 e2e）复用。

## 依赖

- 外部：`rmcp`（1.7，client/server/macros/transport-child-process/transport-io/**transport-streamable-http-client-reqwest**）、
  `tokio`、`thiserror`、`tracing`、`serde_json`、`schemars`。
- 内部：`catalog`（摄取目标类型 `ToolDef` / `Catalog`）、`config`（`connect` 层读 `UpstreamConfig` /
  `UpstreamTransport`）。

## 被谁使用

- `gateway`（M1-B.1）：装配 `UpstreamRegistry`，把上游工具摄取进 catalog，并经元工具 `call_tool` 路由到对应
  `UpstreamHandle`。
- `mcpgw serve`（M1-B.2 / M1-C）：`connect::connect_all` eager-connect 所有上游（按 `transport` 分派 stdio 子进程
  或远程 HTTP）、填充注册表，并把 `RebuildTrigger` 接到 `gateway::run_rebuild_worker`。

## 关键不变量

- 命名空间恒为 `{server}__{name}`（沿用 `catalog` 命名空间方案）。
- 单次摄取内 **intra-server first-dupe-wins**：同名工具保留首个、其余 warn+skip。
- 注册表成员仅在显式 `insert`/`remove`（连接建立/断开）时变化；`get`/`server_names` 不改变成员。

## 向下导航

- 内部细节见 L3：[upstream](../L3-details/upstream.md)
- 逐文件 API 见 L4：[mapping](../L4-api/upstream-mapping.md) · [connection](../L4-api/upstream-connection.md) ·
  [connect](../L4-api/upstream-connect.md) · [registry](../L4-api/upstream-registry.md)
