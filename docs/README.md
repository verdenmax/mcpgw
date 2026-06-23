# mcpgw 分层文档（L1–L4）

本目录是 mcpgw 的**产品/代码文档**，按四个层级组织。它与 `docs/superpowers/`（brainstorming
spec、实现 plan、路线图等过程产物）相互独立。

## 四个层级的含义

| 级别 | 记录什么 | 位置 | 粒度 |
|------|----------|------|------|
| **L1** | 整个模块/项目概览 | `docs/L1-overview.md` | 1 篇 |
| **L2** | 各组件（crate）的职责与公开接口 | `docs/L2-components/<crate>.md` | 每 crate 1 篇 |
| **L3** | 各组件的内部细节（算法、数据流、设计权衡、边界） | `docs/L3-details/<crate>.md` | 每 crate 1 篇 |
| **L4** | 逐源文件的 API（每个 `pub` 项的签名/参数/返回/错误） | `docs/L4-api/<crate>-<file>.md` | 每源文件 1 篇 |

## 强制规则（每个开发 task 的 Definition of Done）

> **边写代码边填充对应文档，并随代码在同一个提交里一起提交。**

每完成一块功能，按改动的层次更新对应文档：

- 新增/改动了某个 `pub` 项（函数/类型/方法/字段/错误）→ 更新该文件的 **L4**。
- 改动了某个组件的职责或对外接口 → 更新该 crate 的 **L2**。
- 改动了内部算法、数据结构或数据流 → 更新该 crate 的 **L3**。
- 新增了 crate、或改动了整体架构/数据流 → 更新 **L1**。

代码评审（spec + 质量 双重审查）应把"对应层级文档是否同步更新"作为验收项之一。

## 索引

- **L1**：[L1-overview.md](./L1-overview.md)
- **L2**：[catalog](./L2-components/catalog.md) · [retrieval](./L2-components/retrieval.md) · [embedder](./L2-components/embedder.md) · [chat](./L2-components/chat.md) · [config](./L2-components/config.md) · [mcpgw-cli](./L2-components/mcpgw-cli.md) · [upstream](./L2-components/upstream.md) · [metatools](./L2-components/metatools.md) · [gateway](./L2-components/gateway.md) · [downstream](./L2-components/downstream.md) · [observe](./L2-components/observe.md) · [dashboard](./L2-components/dashboard.md)
- **L3**：[catalog](./L3-details/catalog.md) · [retrieval](./L3-details/retrieval.md) · [config](./L3-details/config.md) · [mcpgw-cli](./L3-details/mcpgw-cli.md) · [upstream](./L3-details/upstream.md) · [metatools](./L3-details/metatools.md) · [gateway](./L3-details/gateway.md) · [downstream](./L3-details/downstream.md) · [dashboard](./L3-details/dashboard.md)
- **L4**：[catalog/lib.rs](./L4-api/catalog-lib.md) · [retrieval/lib.rs](./L4-api/retrieval-lib.md) · [retrieval/embedder.rs](./L4-api/retrieval-embedder.md) · [retrieval/vector.rs](./L4-api/retrieval-vector.md) · [retrieval/hybrid.rs](./L4-api/retrieval-hybrid.md) · [retrieval/subagent.rs](./L4-api/retrieval-subagent.md) · [embedder/lib.rs](./L4-api/embedder-openai.md) · [chat/lib.rs](./L4-api/chat-openai.md) · [config/lib.rs](./L4-api/config-lib.md) · [mcpgw/main.rs](./L4-api/mcpgw-main.md) · [upstream/mapping.rs](./L4-api/upstream-mapping.md) · [upstream/connection.rs](./L4-api/upstream-connection.md) · [upstream/connect.rs](./L4-api/upstream-connect.md) · [upstream/registry.rs](./L4-api/upstream-registry.md) · [metatools/tools.rs](./L4-api/metatools-tools.md) · [metatools/snapshot.rs](./L4-api/metatools-snapshot.md) · [gateway/lib.rs](./L4-api/gateway-lib.md) · [downstream/lib.rs](./L4-api/downstream-lib.md) · [downstream/http.rs](./L4-api/downstream-http.md) · [observe/lib.rs](./L4-api/observe-lib.md) · [observe/audit.rs](./L4-api/observe-audit.md) · [dashboard](./L4-api/dashboard.md)

> 当前文档覆盖 **M0（检索核心 / Plan 1）**、**M1-A（`upstream` 上游 I/O 层）**、**M1-B.1（`metatools` 元工具
> 逻辑 + `gateway` 快照状态/重建）**、**M1-B.2（`downstream` 下游 MCP 服务 + eager-connect/`serve`）**、
> **M1-C（HTTP 双向传输 + 静态 API-Key 鉴权）** 与 **M2-A（异步可插拔检索 + 向量策略 `VectorStrategy` +
> `Embedder`/`CachingEmbedder` + 新 `embedder` crate 的 `OpenAiEmbedder`）**、
> **M2-B（hybrid RRF 融合 BM25+向量；默认仍 bm25）** 与 **M2.T5（subagent 重排策略 `SubagentStrategy`：BM25 预筛
> + 小模型重排 + 透明降级；新增 `ChatModel` 抽象与 `chat` crate 的 `OpenAiChat`；默认仍 bm25，opt-in）** 与
> **M6.T1（结构化调用日志/追踪：新 `observe` crate 的**仅元数据** `CallRecord` + `CallSink` + `TracingSink`，
> 埋点在 `downstream::call_tool`，`mcpgw serve` 注入默认 `[TracingSink]`）** 与
> **M6.T3（审计落库 JSONL：`observe` 的 `JsonlSink` + 专用 OS 线程 writer + `spawn_writer`/`AuditWriter`、
> 有界 channel 满则丢弃、关停优雅 drain+fsync；配置 `[audit]`，std-only，默认关闭）** 与
> **子系统 A（只读可视化 dashboard：新 `dashboard` crate 的 `MetricsSink`（聚合 per-meta-tool 调用/错误/p50/p95/max
> + per-upstream）+ `CallRingSink`（逐条调用环，M1）+ `DiscoveryRingSink`（有界 ring + 可选发现 JSONL）+ history JSONL 回放
> + `build_dashboard_router` 的 20 个 `/api/*`（含 M3 的 `upstreams/{name}`、`tools/{name}`、`traces/{id}` 详情下钻，`calls`/`activity`/`about`，开放只读 `disabled`，及 6 个 Bearer 鉴权的 admin 写：`{upstreams,tools}/{name}/{disable,enable}` + `config` 读改）+ `assets::static_handler` fallback
> 内嵌一个 **Svelte 5 + Vite 构建、rust-embed 内嵌的多视图 hash-路由 SPA**（M2，`dist/` 入库故 cargo 不依赖 node）；
> `observe` 新增 `DiscoveryRecord`/`DiscoverySink` 发现追踪
> 契约；配置 `[dashboard]`，独立 port、localhost、读端点无鉴权（admin 写子系统 B（运行时禁用）+ C（在线改配/上游热重载）经 Bearer、opt-in）、默认关闭）**。注意
> `embedder`、`chat`、`observe` crate 只有 L2 + L4 文档（无独立 L3；`observe` 的细节并入
> `downstream` L3）；`dashboard` 则另有独立 **L3**（进程模型 / 数据源 / 隐私边界 / 直方图算法）。
> `observe` 的 L4 有两篇（`observe-lib.md` 记录形状/sink 契约、`observe-audit.md` 审计落盘）。
> 后续里程碑（M3 OAuth/反向代理、M4 运行时密钥管理、M6.T2 用量指标、M6.T4 code-mode 等）
> 将按上述规则继续补充各层文档。
> 里程碑路线图见 `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`。
