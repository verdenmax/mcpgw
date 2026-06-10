# M1-B.2 设计：downstream MCP server + `mcpgw serve` + list_changed

- **里程碑**: M1-B.2（M1-B 的第二段，承接已合并的 B.1）
- **状态**: 设计已批准，待出实现计划
- **上游 spec**: [`2026-06-09-mcpgw-m1b-gateway-design.md`](./2026-06-09-mcpgw-m1b-gateway-design.md)（B.1 详细 / B.2 概览）

---

## 1. 目标与背景

B.1 已交付**网关读路径的库**：`metatools`（`GatewaySnapshot` + 三个元工具纯函数）、`gateway`
（`GatewayState`：`ArcSwap` 无锁读 + 串行化 `rebuild_snapshot` + 单上游故障隔离），以及 `upstream`
（rmcp 1.7 client、per-call 超时、命名空间摄取、`UpstreamRegistry`、内存 mock testkit）。但这些仍是库——
**还没有真正连接外部 MCP 子进程，也没有把三个元工具作为一个 MCP server 暴露出去**。`mcpgw` CLI 目前只能对
静态 JSON catalog 跑 `search` / `get-details`。

M1-B.2 把网关**接成活的 MCP server**：任意 MCP 客户端经 stdio 连上 mcpgw，只看到三个元工具
（`search_tools` / `get_tool_details` / `call_tool`），网关在背后聚合多个真实 stdio 上游，并在上游
`notifications/tools/list_changed` 时运行期刷新快照。

### 成功标准

1. 聚合 ≥2 个真实 stdio 上游，下游客户端 `tools/list` **恒为 3 个元工具**，稳定不变。
2. 在 stdio 下游传输上跑通 `search → inspect → execute` 全链路。
3. 上游 `list_changed` 后 `search_tools` 能搜到新增/变更工具（快照已重建）。
4. 单个上游连接失败/崩溃/挂起不影响其它（降级启动 + 超时 + 隔离）；下游 server 始终在线。
5. `mcpgw serve` 读配置 → 连上游 → 起 server → 运行至 stdin 关闭 → 优雅关停上游。

### 范围内 / 范围外

- **范围内**：`downstream` crate、`mcpgw serve`、`[server]` 配置段、真实子进程 `connect_all`、
  `GatewayState` 类型化错误 + 重建遥测、并发 ingest（修死锁）、全链路 e2e、list_changed 运行期刷新。
- **范围外（留后续里程碑）**：Streamable HTTP 传输 + API-Key（M1-C）；向量/混合检索（M2）；
  上游 OAuth；Web 控制台。

---

## 2. 架构

### 2.1 Crate 依赖图（B.2 新增）

```
downstream（新） → metatools + gateway + rmcp(server) + tokio
upstream（改）   → + rmcp(transport-child-process) + tokio(process)
mcpgw（改）      → + downstream + gateway + upstream + config + tokio(rt)
config（改）     → + ServerConfig
```

无环。`downstream` 只依赖 `gateway`（拿 `Arc<GatewayState>`）与 `metatools`（调三个纯函数）。
`connect_all` / `UpstreamClientHandler` 放 `upstream`（它已封装 rmcp client + 子进程传输）。

### 2.2 核心组件一句话职责

| 组件 | 职责 |
|------|------|
| `downstream::GatewayServer` | 实现 rmcp `ServerHandler`，对客户端暴露 3 元工具并把 `call_tool` 派发到 `metatools` |
| `upstream::connect_all` | 启动期 eager 连接所有配置上游（降级启动 + 超时 + 隔离），填充 registry |
| `upstream::UpstreamClientHandler` | rmcp `ClientHandler`，收到上游 `list_changed` 时向重建 trigger 发信号 |
| `gateway::GatewayState`（改） | `rebuild_snapshot` 改并发 ingest + per-ingest 超时 + 类型化错误 + 返回遥测 |
| `mcpgw serve` | 装配：配置 → 连上游 → 起 server → 重建 worker → 运行至关闭 |
| `config::ServerConfig`（新） | `[server] stdio = true`（HTTP 留 M1-C） |

---

## 3. 接口设计

### 3.1 `downstream` crate

```rust
/// 对下游 MCP 客户端暴露三个元工具的 rmcp server。
pub struct GatewayServer {
    state: std::sync::Arc<gateway::GatewayState>,
}

impl GatewayServer {
    pub fn new(state: std::sync::Arc<gateway::GatewayState>) -> Self;
}

impl rmcp::ServerHandler for GatewayServer {
    // get_info：启用 tools capability（含 list_changed=true），声明 server 名/版本。
    fn get_info(&self) -> ServerInfo;

    // list_tools：恒返回 3 个元工具，固定 JSON schema，与运行期上游无关。
    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>)
        -> Result<ListToolsResult, McpError>;

    // call_tool：按 request.name 派发：
    //   "search_tools"     → metatools::search_tools(&snap, query, top_k)
    //   "get_tool_details" → metatools::get_tool_details(&snap, name)
    //   "call_tool"        → metatools::call_tool(&snap, state.registry(), name, args)
    // snap = state.snapshot()（load_full，无锁）。
    // MetaError → CallToolResult { is_error: Some(true), content: [text(err)] }。
    // 未知工具名 → McpError（invalid params）。
    async fn call_tool(&self, req: CallToolRequestParams, _: RequestContext<RoleServer>)
        -> Result<CallToolResult, McpError>;
}
```

**三个元工具的固定 schema（B.2 Task 中精确定文）**：

| 工具 | 入参 | 说明 |
|------|------|------|
| `search_tools` | `{ query: string, top_k?: integer }` | 自然语言检索，返回候选工具摘要列表 |
| `get_tool_details` | `{ name: string }` | 取某工具完整定义（限定名） |
| `call_tool` | `{ name: string, arguments?: object }` | 按限定名路由到上游执行 |

> 不变量：`list_tools` 永远只返回这 3 个；"当下相关工具"只经 `search_tools` 的结果体现，绝不进 `tools/list`。

### 3.2 `upstream`：真实连接 + list_changed 句柄

```rust
/// 重建触发信号：携带触发源 server 名（仅用于日志）。
pub type RebuildTrigger = tokio::sync::mpsc::Sender<String>;

/// 安装到每个上游连接上的 rmcp ClientHandler。
/// 收到 tools/list_changed 通知时，向 trigger 发送本上游名（满了则丢弃，worker 会合并）。
pub struct UpstreamClientHandler {
    server: String,
    trigger: Option<RebuildTrigger>, // None = 不关心通知（内存测试默认）
}

/// eager 连接所有配置上游。降级启动：单个失败仅 warn! 跳过；返回连接遥测。
/// 每个连接：connect_stdio_upstream(带连接超时) → with_call_timeout(cfg.call_timeout_ms)
///          → 安装 UpstreamClientHandler(trigger) → registry.insert(Arc)。
pub async fn connect_all(
    registry: &UpstreamRegistry,
    upstreams: &[config::UpstreamConfig],
    trigger: RebuildTrigger,
) -> ConnectSummary;

/// 薄包装：用 rmcp TokioChildProcess 起子进程（command/args/env_passthrough），
/// 装 UpstreamClientHandler 后 serve。
pub async fn connect_stdio_upstream(
    cfg: &config::UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError>;

/// 连接遥测。
pub struct ConnectSummary {
    pub connected: Vec<String>, // 成功的上游名
    pub skipped: Vec<(String, String)>, // (上游名, 错误摘要)
}
```

> rmcp 句柄类型变化：现 `UpstreamHandle.client: RunningService<RoleClient, ()>`。为收通知需把 `()`
> 换成单一具体类型 `UpstreamClientHandler`（`trigger: None` 时为 no-op，保持一个具体 `RunningService`
> 类型，内存测试与真实子进程共用）。`connect` 改为接收该 handler。**该 API 精确形态由 B.2 Task 1 spike 锁定。**

### 3.3 `gateway`：类型化错误 + 重建遥测 + 修死锁

```rust
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("unknown retrieval strategy: {0}")]
    Strategy(String),
}

/// 一次重建的结果遥测。
pub struct RebuildSummary {
    pub ingested: Vec<String>,           // 成功摄取的上游名
    pub skipped: Vec<(String, String)>,  // (上游名, 失败原因：超时/调用错误)
}

impl GatewayState {
    pub fn new(strategy_name: &str) -> Result<Self, GatewayError>; // String → GatewayError

    /// 并发 ingest 所有上游，每个 ingest 包 tokio::time::timeout(call_timeout)；
    /// 超时/失败的上游计入 skipped 并跳过（隔离）；其余构建进新快照后 ArcSwap 切换。
    /// 全程持 rebuild_lock 串行化重建；读者无锁。
    pub async fn rebuild_snapshot(&self) -> Result<RebuildSummary, GatewayError>;
}
```

**修 `m1b2-ingest-timeout`（[Important] 死锁）**：B.1 的 `rebuild_snapshot` 顺序 `await` 每个
`ingest_into`（内部 `list_all_tools()` 无超时），一个"已连接但静默"的上游会让 ingest 永久挂起并占住
`rebuild_lock`，饿死后续所有重建（含 list_changed 触发）。B.2 改为：

- **并发 ingest**：`futures::future::join_all` 同时摄取各上游到各自的本地结果。
- **per-ingest 超时**：每个 ingest 包 `tokio::time::timeout(handle.call_timeout())`；elapsed = 计入
  `skipped` 并跳过。需给 `UpstreamHandle` 暴露 `call_timeout()` getter（或新增 `ingest_into` 超时参数）。
- 合并所有成功结果到一个 `Catalog`（保持 intra-server first-dupe-wins 语义），再 build+index+swap。

> 并发 ingest 写各自局部 catalog 再合并（而非共享 `&mut Catalog`），避免跨 await 的可变借用问题。

### 3.4 `config`：`[server]` 段

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub stdio: bool, // 默认 true；HTTP 留 M1-C
}

// Config 新增： #[serde(default)] pub server: ServerConfig
```

`ServerConfig` 不含 `#[serde(flatten)]`，故可加 `deny_unknown_fields`（与 `RetrievalConfig` 一致）。

---

## 4. 数据流 / 生命周期

### 4.1 `mcpgw serve`

```
serve(config_path):
  cfg = Config::from_toml_str(...)          # 非法 TOML → fail-fast 退出
  state = Arc::new(GatewayState::new(cfg.retrieval.strategy)?)
  (tx, rx) = mpsc::channel::<String>(N)     # 重建 trigger
  summary = connect_all(state.registry(), &cfg.upstreams, tx.clone()).await
  log!("upstreams: connected={..} skipped={..}", summary)   # 降级启动：可能 0 连接
  state.rebuild_snapshot().await?           # 初始快照（log RebuildSummary）
  spawn rebuild_worker(state.clone(), rx)   # 见 4.3
  server = GatewayServer::new(state.clone())
  service = server.serve(stdio()).await?    # 下游 stdio
  service.waiting().await                    # 运行至 stdin EOF / 客户端断开
  graceful: registry 各 handle.shutdown().await
```

### 4.2 一次 `call_tool` 调用（下游视角）

```
client --tools/call name="call_tool" args={name:"github__create_issue", arguments:{...}}--> GatewayServer
  GatewayServer.call_tool:
    snap = state.snapshot()                      # load_full，无锁
    metatools::call_tool(&snap, state.registry(), "github__create_issue", args)
      → snap.catalog.get(name) → ToolDef{server:"github", name:"create_issue"}  # 不拆 "__"
      → registry.get("github").call_tool("create_issue", args)  # 带 per-call 超时
    Ok(CallToolResult) | MetaError → CallToolResult{is_error:true}
```

### 4.3 list_changed 运行期刷新（approach A：trigger 通道 + 重建 worker）

```
上游 X --notifications/tools/list_changed--> UpstreamClientHandler(server="X")
  → tx.try_send("X")            # 通道满则丢弃（worker 会合并这次突发）

rebuild_worker(state, rx):
  loop:
    name = rx.recv().await      # 阻塞等第一条
    drain rx（try_recv 清空）    # 合并突发：一段时间内多条通知 → 一次重建
    state.rebuild_snapshot().await   # 串行化由 rebuild_lock 保证
    log!(RebuildSummary)
```

合并（coalesce）避免上游连发通知导致 N 次重建；重建本身仍由 `rebuild_lock` 串行化（last-store-wins）。

---

## 5. 错误处理

| 场景 | 行为 |
|------|------|
| 配置 TOML 非法 | `serve` 启动 fail-fast，非零退出 |
| 检索策略未实现 | `GatewayState::new` 返回 `GatewayError::Strategy`，`serve` fail-fast |
| 上游连接失败/超时（启动期） | `connect_all` warn! + 计入 `skipped`，**继续启动**（降级） |
| 全部上游连接失败 | server 仍启动（空工具集）；`search_tools` 返回空；下游在线 |
| 上游 ingest 挂起（重建期） | per-ingest 超时 → 计入 `RebuildSummary.skipped` → 跳过；不阻塞其余/不占锁 |
| `call_tool` 上游超时 | `UpstreamError::Timeout` → `MetaError::Timeout` → `CallToolResult{is_error:true}` |
| `call_tool` 工具不存在 | `MetaError::ToolNotFound` → `CallToolResult{is_error:true}` |
| 下游未知工具名 | `McpError`（invalid params） |
| 超时后上游残留响应（`m1b2-cancel-note`） | spike 核实 rmcp 优雅丢弃陈旧响应；不主动发 `notifications/cancelled`（YAGNI），除非 spike 证实泄漏 |

---

## 6. 测试策略

**Task 1 = spike**（锁定 rmcp 风险，不进生产路径）：核实 rmcp 1.7 动态 `ServerHandler`
（`get_info`/`list_tools`/`call_tool` 精确签名）与 `ClientHandler` 的 list_changed 通知钩子方法名/签名；
确认超时丢弃 future 后上游残留响应被优雅吞掉。spike 结论写入计划。

随后逐 task TDD（每个 task：失败测试 → 最小实现 → 绿 → L1-L4 文档同提交 → commit）：

| 验证点 | 测试形态 |
|------|------|
| `list_tools` 恒返回 3 元工具 + schema | downstream 单测：断言名字集合 = {search_tools,get_tool_details,call_tool}、schema 字段 |
| `call_tool` 派发 + isError 映射 | downstream 单测：mock state，三种工具名各派发一次；`MetaError` → `is_error:true` |
| `connect_all` 降级启动 + 遥测 | upstream 集成测试（testkit）：2 个 mock + 1 个坏命令 → `connected=2, skipped=1`，server 仍可用 |
| 并发 ingest + per-ingest 超时（修死锁） | gateway 集成测试：注入一个 hung mock → 重建不挂起，hung 入 `skipped`，其余工具在新快照 |
| 全链路 e2e | rmcp 测试 client →（内存 duplex）→ GatewayServer → mock 上游：`search → get_details → call_tool(echo)` 成功 |
| list_changed 刷新 | mock 上游触发 `list_changed`（或重连暴露新工具）→ 等重建 → `search_tools` 搜到新工具 |

**CI 约束**：真实子进程 spawn（`connect_stdio_upstream` 的 `TokioChildProcess` 路径）只在一个轻量
**冒烟测试**里验证（例如用 `cat`/echo 脚本或自身的一个 mock binary），避免 CI 依赖外部真实 MCP server；
核心逻辑全部走内存 duplex mock。

---

## 7. 开工前在实现计划里细化的点

- Task 1 spike 后定：rmcp `ClientHandler` 通知钩子精确 API；`UpstreamHandle` 句柄类型从 `()` 迁到
  `UpstreamClientHandler` 的最小改动面（是否需要泛型/类型擦除）。
- 三个元工具的 JSON schema 精确字段（`top_k` 是否必填、`arguments` 形态）。
- `connect_stdio_upstream` 的 `TokioChildProcess` 精确构造 + `env_passthrough` 透传方式（参考 M1-A 客户端例）。
- 重建 worker 的合并窗口：先实现"drain 即合并"（无定时 debounce），按需再加。
- e2e 的下游测试传输：优先内存 duplex（快、确定）；真实 stdio 子进程仅冒烟。

---

## 8. 关联文档

- 上游设计：[M1-B 网关设计](./2026-06-09-mcpgw-m1b-gateway-design.md)
- 渐进式发现总设计：[progressive-discovery-design](./2026-06-08-mcpgw-progressive-discovery-design.md)
- 实现计划（待写）：`docs/superpowers/plans/2026-06-10-mcpgw-m1b2-downstream-serve.md`
- 分级文档（随实现填充）：`docs/L2-components/downstream.md`、`docs/L3-details/downstream.md`、
  `docs/L4-api/downstream-*.md`，并更新 `config`/`gateway`/`upstream` 的 L3/L4 与 `L1-overview.md`。
