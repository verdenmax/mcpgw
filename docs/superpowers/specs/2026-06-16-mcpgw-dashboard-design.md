# mcpgw 只读可视化面板（dashboard）设计

日期：2026-06-16
状态：已批准设计（待写实施计划）

## 背景与目标

mcpgw 目前只有 CLI + MCP 协议端点，没有任何人看的运行时面板；可观测性仅靠 stderr 结构化日志与可选 JSONL 审计。
本设计新增一个**只读 Web 可视化面板**，随 `mcpgw serve` 在独立端口拉起，让运维者能在浏览器里：

- 看当前接入的所有上游 MCP 及其连接状态、工具数、调用数、错误率；
- 看聚合调用率 / 延迟 / 成功率（实时 + 历史回放）；
- 看每个 `search_tools` query **选中了哪些 tool（含相关性分数）**（实时 + 可选历史回放）。

**明确范围**：本期只做**只读可视化（子系统 A）**。在线改配置 / 启停上游（写操作，子系统 B）与当前"启动期静态
fail-fast 配置 + 不可变快照"架构有根本冲突（路线图 M4），**不在本期**，将另立项。面板**仅 localhost、暂不鉴权**。

## 架构与进程模型

新增 crate **`dashboard`**（隔离 UI/HTTP，保持核心 crate 干净），由 `mcpgw serve` 在**独立 tokio 任务 + 独立端口**
拉起，与现有 stdio / HTTP 下游正交、可单独开关：

```
mcpgw serve（一个进程，共享 Arc<GatewayState> + observe sinks）
├─ stdio 下游任务         （已有）
├─ HTTP 下游任务 :8970    （已有，MCP /mcp 端点）
└─ dashboard 任务 :8971   （新增，独立端口，仅 localhost）
     ├─ panic 边界：任务体捕获 panic，只记 error，不波及其它任务
     ├─ 只读：Arc<GatewayState> 快照 + 内存 MetricsSink/DiscoverySink 句柄 + 审计/discovery 文件
     └─ axum Router：静态前端 + /api/* JSON（with_graceful_shutdown，随主 select 优雅关停）
```

- **依赖方向**：`dashboard → {gateway, observe, catalog}` + `axum`/`tokio`/`serde`/`serde_json`；不被任何核心 crate 反向依赖。
- **隔离**：独立任务 + 独立端口 + panic 边界 + 所有缓冲有界（满则丢/覆盖，与 `JsonlSink` 一致）——以单进程内的任务隔离
  吸收"面板拖垮网关"的顾虑。
- **关停**：纳入现有 `tokio::select!`，oneshot 驱动 `with_graceful_shutdown`（与 http_task 同模式）。
- **默认关闭**：`[dashboard].enabled=false`，须显式 opt-in。

## 数据模型与捕获

**核心原则：保持现有 `CallRecord` 仅元数据不变**（继续喂 `TracingSink`/`JsonlSink`，绝不泄露 query/工具名）。
富追踪走**独立、显式 opt-in 的通道**，与隐私洁净的 `CallRecord` 物理隔离。

### MetricsSink（实现已有 `observe::CallSink` 接缝，即规划中的 M6.T2）
- 进程内聚合每条 `CallRecord`：按 `(meta_tool, outcome)` 维度的计数、错误数、延迟（sum/max + 简单分桶以算 p50/p95）、
  字节量；并按 `CallRecord.upstream` 维度分桶（供每上游调用数/错误率）。
- 桶数有限（3 元工具 × 少量 outcome + 上游数）→ **内存天然有界**。
- 暴露 `fn snapshot(&self) -> MetricsSnapshot`（克隆出当下聚合值）；内部 `Mutex`，锁不跨 `.await`。
- 放在 `crates/dashboard/src/metrics.rs`。

### DiscoverySink（`observe` 新增契约 trait，仅当 `trace_queries=true` 挂载）
- 契约（放 `observe`，与 `CallSink`/`CallRecord` 并列）：
  ```rust
  pub struct DiscoveryRecord {
      pub ts_unix_ms: u64,
      pub query: String,
      pub top_k: usize,
      pub results: Vec<DiscoveryHit>,   // { name: String, score: f32 }
      pub latency_ms: u64,
  }
  pub trait DiscoverySink: Send + Sync { fn record(&self, rec: &DiscoveryRecord); }
  ```
- 捕获落点：`downstream::GatewayServer::call_tool` 的 `search_tools` 分支（那里已有 `query` 与命中列表）。**只在
  `trace_queries` 开启、且注入了非空 discovery sinks 时构造**；`metatools` 保持纯函数、不依赖 `observe`。
  `downstream::GatewayServer::new` 与 `downstream::http::build_router` **新增一个 discovery-sinks 参数**
  （`Arc<[Arc<dyn observe::DiscoverySink>]>`，默认空切片表示不捕获），与现有 `CallSink` 切片并列注入。
- 具体实现 `DiscoveryRingSink`（放 `crates/dashboard/src/trace.rs`）：**内存 ring buffer**（最近 `trace_buffer` 条，
  满则覆盖最旧）给实时；当配置 `trace_path` 时，**额外**把每条 `DiscoveryRecord` 序列化为一行 JSON 经**有界 channel +
  后台 writer 线程**追加到 discovery JSONL（复用 `JsonlSink` 同款 std-only writer 模式，满则计数丢弃、不阻塞调用）。

### 分数贯通
`metatools::search_tools` 当前把检索分数丢弃（`retrieval::ScoredTool { qualified_name, description, score }` →
`ToolSummary { name, description }`）。设计**给 `ToolSummary` 增加 `score: f32` 字段**：
- 对 MCP 客户端**向后兼容**（JSON 加字段），且客户端也能看到相关性分数（顺带改进）；
- `search_tools` 响应与 discovery 追踪共用这一份带分数的命中（`downstream` 从同一 `Vec<ToolSummary>` 构造 `DiscoveryRecord`）。

### 历史回放（按需、有界）
- **历史指标**：dashboard 读已有**审计 JSONL**（`[audit].enabled` 时存在，含 `ts_unix_ms`/`meta_tool`/`outcome`/
  `latency_ms`）→ 分时段桶重建历史调用率/延迟。
- **历史 query→tools**：读新增的 **discovery JSONL**（`trace_path` 配置时存在）。
- 两者均**限量读取**（尾部 N 行 / 近时间窗），不无界载入；文件缺失/坏行 → 跳过并返回 `history_unavailable` 标志，不报错。

### 数据来源汇总

| 视图 | 实时来源 | 历史来源 |
|---|---|---|
| 上游/工具清单 + 状态 | `Arc<GatewayState>` 快照 | —（当前态） |
| 调用率 / 延迟 / 成功率 | `MetricsSink`（内存） | 审计 JSONL 回放（需 `[audit].enabled`） |
| query→tools + 分数 | `DiscoveryRingSink` ring buffer | discovery JSONL 回放（需 `trace_path`） |

## HTTP JSON API（全部只读 GET，dashboard 端口，localhost）

| 端点 | 内容 | 来源 |
|---|---|---|
| `GET /api/overview` | 运行时长、当前 strategy、上游 up/down 数、工具总数、上次重建时间、累计调用数 | 快照 + MetricsSink |
| `GET /api/upstreams` | 每上游：name、transport、状态（connected / skipped+原因）、工具数、调用数、错误率 | 快照 + MetricsSink（按 `upstream` 分桶） |
| `GET /api/tools?q=` | 聚合后的工具清单（可选关键词过滤） | 快照 catalog |
| `GET /api/metrics` | 每元工具：调用数、错误率、p50/p95 延迟 | MetricsSink |
| `GET /api/traces?limit=N&source=live\|history` | query→tools+分数追踪 | DiscoveryRingSink / discovery JSONL |
| `GET /api/metrics/history` | 历史调用率/延迟（分时段桶） | 审计 JSONL 回放（有界） |

- 所有响应 `application/json`；错误以 `{ "error": ... }` + 合适状态码返回，但历史不可用是**正常空响应 + `history_unavailable` 标志**，非 500。
- handlers 只读无锁快照 + 短 `Mutex`（不跨 `.await`）；历史文件读限量。

## 前端 SPA（纯 vanilla JS，零构建、零框架、零图表库）

- 静态资源 `index.html` + `app.js` + `style.css`，经 `include_str!` 内嵌进 `dashboard` crate（**不引 rust-embed**），
  由 dashboard 的 axum router 直接 serve（`/` → index.html，`/app.js`、`/style.css`）。
- 四个面板：
  1. **Overview** 卡片（运行时长 / strategy / 上游 up·down / 工具总数 / 累计调用）。
  2. **Upstreams 表**（名称 / transport / 状态徽章 / 工具数 / 调用数 / 错误率）。
  3. **Metrics**（每元工具调用数·错误率·p50/p95，用 **CSS 条形**呈现，不引图表库）。
  4. **Query Traces**（实时列表：时间 / query 原文 / 返回工具名+分数，可展开；live/history 切换）。
- **实时更新**：前端**定时轮询** `/api/*`（默认每 3s）。不上 SSE/WebSocket。

## 配置 `[dashboard]` 段

`Config.dashboard: DashboardConfig`（`#[serde(default, deny_unknown_fields)]`，无 flatten）：

```toml
[dashboard]
enabled = false                       # 须显式 opt-in（默认 false）
bind = "127.0.0.1:8971"               # 默认 localhost:8971
trace_queries = false                 # opt-in 捕获 query 原文 + 工具名 + 分数（默认 false）
trace_path = "mcpgw-discovery.jsonl"  # 省略=只内存 ring buffer（无历史回放）；默认 None
trace_buffer = 500                    # ring buffer 条数（默认 500）
```

- `validate()`：`enabled` 时 `bind` 非空且可解析；`bind` 非 loopback → 醒目 `tracing::warn!`（面板无鉴权且含 query
  原文，沿用 N2 的**非破坏式 warn**，不拒绝启动）。`trace_buffer` 须 `> 0`。
- 各字段默认见上；省略整个 `[dashboard]` 段 → `DashboardConfig::default()`（关闭）。

## 错误处理 / 隔离

- dashboard 任务裹 **panic 边界**：崩溃只记 error，不波及 stdio/HTTP/网关其它任务。
- 所有缓冲**有界**：ring buffer 覆盖最旧；discovery JSONL 走有界 channel 满则丢（同 `JsonlSink`）。
- API **不阻塞网关**：读无锁 ArcSwap 快照 + 对 MetricsSink/ring 的短 `Mutex`（绝不跨 `.await`）；历史文件**限量读**，
  文件错误 → 空 + `history_unavailable` 标志。
- 关停：纳入主 `tokio::select!`（oneshot 驱动，与 http_task 同），优雅关停后 discovery writer 干净 drain。

## 测试

- **单元**：MetricsSink 聚合（计数 / outcome / p50·p95 / 按 upstream 分桶）；ring buffer 有界覆盖；`DiscoveryRecord`
  由一次 search 正确构造（名+分数有序）；`ToolSummary` 分数贯通（search_tools 带出 score）；`[dashboard]` 解析/校验
  （默认值、`deny_unknown_fields`、非 loopback warn、`trace_buffer>0`）。
- **API**：seed 一个 GatewayState + MetricsSink + DiscoveryRingSink，逐端点断言 JSON 形状（axum/tower `oneshot`）；
  历史不可用返回 `history_unavailable`。
- **历史回放**：小 JSONL fixture → 分桶正确；坏行跳过、限量生效。
- **e2e**（testkit，默认 ignored，与现有冒烟一致）：起 `mcpgw serve` 开 dashboard，一次 search 后打 `/api/overview`
  与 `/api/traces?source=live` 断言含该 query。
- **前端**：逻辑薄；冒烟断言 `GET /` 返回 200 + `text/html`，`/app.js` 200。

## crate 布局

```
crates/dashboard/
  Cargo.toml            # deps: gateway, observe, catalog, axum, tokio, serde, serde_json, tracing
  src/lib.rs            # build_dashboard_router(state, metrics, discovery, cfg) + API handlers
  src/metrics.rs        # MetricsSink (impl observe::CallSink) + MetricsSnapshot
  src/trace.rs          # DiscoveryRingSink (impl observe::DiscoverySink) + 可选 discovery writer
  src/history.rs        # 有界读取/回放 审计 & discovery JSONL
  assets/index.html
  assets/app.js
  assets/style.css
```
- `observe` 新增 `DiscoveryRecord` + `DiscoverySink` trait（契约层）。
- `metatools`：`ToolSummary` 加 `score: f32`，`search_tools` 带出分数（纯函数不变性保持）。
- `config`：新增 `[dashboard]` 段。
- `mcpgw`：装配——当 `[dashboard].enabled` 时构造 `MetricsSink`（作为 CallSink 之一加入 sinks 切片）、按
  `trace_queries` 构造 `DiscoveryRingSink`，起 dashboard 任务并入 `select!`；把 discovery sinks 注入 stdio + HTTP
  两条下游（dashboard 关闭时不构造这些、下游收到空 discovery 切片，零额外开销）。

## 范围外（YAGNI → 未来 / 子系统 B）

- 在线改配置 / 启停上游（写操作）—— 子系统 B，另立项。
- 鉴权 —— 本期仅 localhost。
- SSE/WebSocket —— 用轮询。
- 图表库 —— CSS 条形 / 内联。
- 多网关聚合、历史超出 opt-in discovery JSONL 之外的来源。

## 交付

按本仓库一贯工作流：subagent 实现 + 每 task spec+质量双重审查、折叠 nit、最终整分支 `code-review`、`--no-ff` 本地合并、
复测、删分支、推送 origin；分层文档 L1–L4 同步、L1 测试计数更新。
