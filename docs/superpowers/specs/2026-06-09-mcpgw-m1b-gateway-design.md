# mcpgw M1-B —— Gateway 状态 + Meta-tools（+ Downstream）设计

- **状态**: 已批准设计，待编写 B.1 实现计划
- **日期**: 2026-06-09
- **承接**: M1 整体设计 `2026-06-08-mcpgw-m1-live-gateway-design.md`；M1-A（upstream 地基，已合并）
- **里程碑**: M1-B（M1 的第二段），拆为 **B.1** 与 **B.2**

---

## 1. 背景与范围

M1-A 交付了 `upstream` crate（连上游 MCP、命名空间摄取、`call_tool` 转发、`UpstreamRegistry`、内存
mock 测试夹具）。**M1-B 把它接成"网关大脑 + 对下游的 MCP server"**，最终让任意 MCP 客户端连上 mcpgw、
只看到 3 个元工具、背后聚合真实上游。

按"增量去风险"拆分（与 M1-A 同法）：

- **M1-B.1（本 spec 详细部分）**：`gateway` 状态（ArcSwap 快照 + eager 连接 manager + 超时）+ `metatools`
  三个元工具函数。**不碰 rmcp server**——用已验证的 upstream client + 自有 ArcSwap/检索逻辑，可用内存 mock
  完整测试。
- **M1-B.2（概览，后续单独出计划）**：`downstream` rmcp `ServerHandler`（stdio）暴露三元工具 + `mcpgw serve`
  命令 + 全链路 e2e + **list_changed 运行期刷新**（B.2 最后一个 task）。rmcp server 风险集中于此，B.2 首个
  task 用 spike 锁定动态 `ServerHandler` API。

### 已敲定的 M1-B 决策

| 决策 | 内容 |
|------|------|
| rmcp 动态 server API | override `ServerHandler::{list_tools, call_tool}`（`async`，`RequestContext<RoleServer>`，返回 `ListToolsResult`/`CallToolResult`）。已核实 rmcp 1.7 `handler/server.rs` 签名 |
| list_changed | 含运行期刷新，作为 **B.2 最后一个 task**（先端到端跑通再加） |
| 拆分 | B.1（状态+metatools）/ B.2（downstream+serve+list_changed），先做 B.1 |
| call_tool 路由 | **用 catalog 查 `ToolDef`** 取 `server`+`name` 字段路由，**不做 `__` 字符串拆分**（收编最终评审遗留项） |
| 超时 | 给 `UpstreamHandle` 加 `call_timeout`，`call_tool` 内部 `tokio::time::timeout` 包裹（收编 M1-A 遗留项） |

---

## 2. M1-B.1 详细设计

### 2.1 新增 crate（无环依赖）

```
crates/
├─ metatools/   # GatewaySnapshot 类型 + 三个元工具函数
│               # 依赖 catalog/retrieval/upstream + rmcp(CallToolResult)/serde_json/thiserror
└─ gateway/     # GatewayState(Arc<ArcSwap<快照>> + UpstreamRegistry) + 快照构建/刷新 + eager 连接
                # 依赖 metatools/upstream/catalog/retrieval/config + arc-swap/rmcp(TokioChildProcess)/tokio
```

依赖方向：`gateway → metatools → {catalog, retrieval, upstream}`；`gateway → {upstream, config, arc-swap}`。
B.2 再加 `downstream`（依赖 `metatools` + `upstream` + `arc-swap` + `rmcp`）。无环。

### 2.2 `metatools` crate

- **`GatewaySnapshot { catalog: Catalog, strategy: Box<dyn RetrievalStrategy> }`** —— ArcSwap 装载的不可变
  快照（catalog + 已 `index` 的策略）。
- **`ToolSummary { name: String, description: String }`** —— `search_tools` 的返回项。
- **`MetaError`**（thiserror）—— `ToolNotFound(String)` / `UpstreamUnavailable(String)` / `Timeout(String)` /
  `Call(...)`（下游 server 在 B.2 把它映射成 MCP `isError`）。
- 三个纯函数（B.2 的 `ServerHandler::call_tool` 会调用它们）：
  - `pub fn search_tools(snap: &GatewaySnapshot, query: &str, top_k: usize) -> Vec<ToolSummary>`
    —— `snap.strategy.search(query, top_k)` → 映射成 `{name, description}`。
  - `pub fn get_tool_details<'a>(snap: &'a GatewaySnapshot, name: &str) -> Option<&'a catalog::ToolDef>`
    —— `snap.catalog.get(name)`。
  - `pub async fn call_tool(snap: &GatewaySnapshot, registry: &UpstreamRegistry, name: &str,
    args: Option<serde_json::Map<String, serde_json::Value>>) -> Result<rmcp::model::CallToolResult, MetaError>`
    —— `snap.catalog.get(name)` 取 `ToolDef{server, name}`；`registry.get(def.server)` 取句柄；
    `handle.call_tool(def.name, args)`（句柄内部已带超时）；找不到工具/上游 → `MetaError`。

### 2.3 `gateway` crate

- **`GatewayState { snapshot: Arc<ArcSwap<GatewaySnapshot>>, registry: UpstreamRegistry, strategy_name: String }`**。
- **`rebuild_snapshot(&self)`** —— 遍历 `registry` 各上游 `ingest_into` 一个新 `Catalog` →
  `retrieval::build_strategy(&self.strategy_name)` + `index` → `ArcSwap.store(Arc::new(新快照))`。读端永不阻塞。
- **`connect_all(&self, upstreams: &[config::UpstreamConfig])`** —— **eager**：对每个 stdio 上游
  `connect_stdio_upstream(cfg)`，**带 `call_timeout_ms` 超时 + 故障隔离**（单个失败只 `warn!` 跳过），成功的
  插入 `registry`；最后 `rebuild_snapshot()`。
- **`connect_stdio_upstream(cfg: &UpstreamConfig) -> Result<UpstreamHandle, _>`** —— 薄包装：用 rmcp
  `TokioChildProcess::new(Command::new(cfg.command).args(cfg.args).envs(透传白名单))` 起子进程，
  `UpstreamHandle::connect(cfg.name, child)`，并把 `cfg.call_timeout_ms` 设进句柄。

### 2.4 `upstream` 改动（收编 M1-A 超时遗留项）

给 `UpstreamHandle` 加 `call_timeout: Duration`（`connect` 时缺省、或经 setter 从 cfg 设），`call_tool`
内部 `tokio::time::timeout(self.call_timeout, ...)` 包裹；超时映射为 `UpstreamError`（metatools 再转 `MetaError::Timeout`）。

### 2.5 数据流（B.1 可测范围）

```
connect_all(config) → 各上游 connect_stdio(带超时/隔离) → registry
rebuild_snapshot()  → 各 ingest_into → build+index 策略 → ArcSwap.store
search_tools(snap,q)        → snap.strategy.search → [{name, description}]
get_tool_details(snap,name) → snap.catalog.get → &ToolDef
call_tool(snap,reg,name,a)  → snap.catalog.get(name)→ToolDef → reg.get(server).call_tool(orig,a)[带超时]
```

**关键不变量**：下游（B.2）看到的工具列表恒为 3 元工具；"当下相关工具"只经 `search_tools` 体现。

### 2.6 错误处理

| 场景 | 处理 |
|------|------|
| 上游连接失败 | 隔离：`warn!` 跳过，不影响其它；`rebuild` 只纳入已就绪上游 |
| `call_tool` 工具不存在 | `MetaError::ToolNotFound`（B.2 → MCP `isError`） |
| 目标上游缺失/已断 | `MetaError::UpstreamUnavailable` |
| 调用超时 | `MetaError::Timeout` |
| 配置非法 | `serve` 启动 fail-fast（B.2） |

### 2.7 测试策略（全部用 M1-A 的内存 mock，不需真实子进程）

- **metatools**：连 mock 上游 → 建 `GatewaySnapshot` → 断言 `search_tools` 命中、`get_tool_details` 返回
  schema、`call_tool` 转发到 mock `echo` 成功；路由经 catalog 查（不拆字符串，构造一个 server 名不含 `__`
  但 tool 名含 `__` 的用例，证明拆字符串会错而我们不错）。
- **gateway**：`GatewayState` 注入 mock 句柄进 `registry` → `rebuild_snapshot` → 断言快照含命名空间工具且
  可搜；`ArcSwap` 替换后旧 `load()` 的读者仍能用旧快照（不阻塞、不崩）。
- **超时**：mock 一个会挂起的工具调用 → 断言 `call_tool` 在超时内返回 `MetaError::Timeout`。
- `connect_all` 的真实子进程 spawn 路径是薄包装，放 **B.2 冒烟**验证（避免 CI 依赖真实 MCP）。
- **L1–L4 文档**随每个 task 同步提交，纳入双重审查验收。

---

## 3. M1-B.2 概览（后续单独出计划）

- **`downstream` crate**：`GatewayServer` 实现 rmcp `ServerHandler`：`get_info`（启用 tools capability）、
  `list_tools` 恒返回 3 个元工具及其固定 JSON schema、`call_tool` 按 `request.name` 派发到 `metatools` 的三个
  函数；`MetaError` → `CallToolResult { is_error: true }`。传输：`serve(stdio())`。
- **`mcpgw serve` 命令**：读 config → `GatewayState::connect_all` → 启动 downstream stdio server → 运行至关闭。
- **`[server]` 配置段**：M1-B.2 加 `[server] stdio = true`（HTTP 留 M1-C）。
- **全链路 e2e**：rmcp 测试客户端 → 网关（stdio）→ mock 上游，断言 `search→inspect→execute`。
- **list_changed（B.2 最后一个 task）**：给上游连接装自定义 `ClientHandler` 接收
  `notifications/tools/list_changed` → 触发 `GatewayState::rebuild_snapshot()` → 断言 `search_tools` 能搜到
  新增/变更工具。

---

## 4. 成功标准（M1-B 整体）

- 聚合 ≥2 个上游，下游客户端仅见 3 个元工具，`tools/list` 稳定不变。
- 在 stdio 下游传输上跑通 `search → inspect → execute` 全链路。
- 上游 `list_changed` 后 `search_tools` 能搜到变更工具（快照已重建）。
- 单个上游崩溃/挂起不影响其它（超时 + 隔离）；下游始终在线。

## 5. 开工前在实现计划里细化的点

- B.1：`UpstreamHandle` 超时改动是否需要改 `connect` 签名 vs 加 setter（计划首个 task 定）。
- B.1：`connect_stdio_upstream` 的 env 透传与 `TokioChildProcess` 精确构造（参考 M1-A 客户端例）。
- B.2：rmcp 动态 `ServerHandler` 的精确形态（B.2 Task1 spike 锁定）；元工具 JSON schema 的精确字段。
