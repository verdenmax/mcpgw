# mcpgw 程序级路线图（Milestone → Tasks）

- **状态**: 规划中（活文档，随里程碑推进更新）
- **日期**: 2026-06-08
- **关联**: `2026-06-08-mcpgw-progressive-discovery-design.md`（spec）、`2026-06-08-mcpgw-retrieval-core.md`（Plan 1）

## 如何使用本文档

- 本文是**程序级路线图**：把整个项目拆成里程碑（M0–M6）+ 横切工作，每个里程碑拆成多个 task。
- **粒度**：这里的 task 是"一块可独立交付/测试的工作"，不是逐步骤 TDD。每个里程碑**正式开工时**，用
  `writing-plans` 技能为它生成一份逐步骤 TDD 计划（像 Plan 1），再用 subagent-driven-development 执行。
- **里程碑编号已重排**：spec 的 P1–P6 与 Plan 1 的"Plan 2 hand-off"有重叠，这里统一为 M0–M6 并给出依赖关系。

## 里程碑全景与依赖

```
M0 ✅ 检索核心(Plan 1)  ──►  M1 活 MCP 网关(I/O 层) ──►  ┬─► M2 检索深度(向量/混合/subagent)
                                                          ├─► M3 公网暴露(隧道/反代/OAuth)
                                                          ├─► M4 控制面板(Web/移动) ──► M5 RBAC+审批
                                                          └─► M6 可观测性/审计 + code-mode
横切(持续): CI · 打包发布 · 文档(L1–L4) · 安全
```

- **M1 是一切的前提**：没有活 I/O 层，mcpgw 还不是一个能用的网关。
- M2/M3/M4/M6 在 M1 之后大体并行；M5 依赖 M4；M4 的"看调用"依赖 M6 的部分能力。

| 里程碑 | 主题 | 依赖 | 产出（做完后什么能用） |
|--------|------|------|------------------------|
| **M0** ✅ | 检索核心 | — | 库 + CLI：加载工具目录、BM25 检索 |
| **M1** | 活 MCP 网关 | M0 | 任意 MCP 客户端连上 mcpgw，得到 3 个元工具，背后聚合 N 个真实上游 MCP |
| **M2** | 检索深度 | M1 | 向量/混合/subagent 可插拔策略，默认 BM25+向量 |
| **M3** | 公网暴露 | M1 | 一键安全暴露到外网（隧道 + 反代不踩坑 + OAuth/API-Key） |
| **M4** | 控制面板 | M1（+M6 看调用） | Web/移动端：分组、启停、扫码分享、看调用 |
| **M5** | RBAC + 审批 | M4 | 按 key/用户/工具的细粒度权限 + 危险操作审批拦截 |
| **M6** | 可观测/审计 + code-mode | M1 | 调用日志/追踪/用量；沙箱内 programmatic tool calling |

---

## M0 — 检索核心（Plan 1）✅ 已完成

四 crate 工作区（catalog/retrieval/config/mcpgw）、自研 BM25、可插拔 `RetrievalStrategy`、`[retrieval]`
配置、search/get-details CLI。21 测试绿、clippy 净。详见 Plan 1 文档。

---

## M1 — 活 MCP 网关（I/O 层）★ 最高优先

**目标**：任意 MCP 客户端连上 mcpgw → 只看到 `search_tools`/`get_tool_details`/`call_tool` 三个元工具 →
背后聚合 N 个真实上游 MCP（stdio + Streamable HTTP）。这是把 M0 的检索核心变成真正网关的关键一跳。

**新增 crate**：`upstream`、`downstream`、`metatools`（+ `router` 模块）；扩展 `config`、`mcpgw`。

**Tasks**
- **M1.T1 — rmcp 选型 spike**：定 rmcp 版本，跑通 rmcp 的 client + server 最小例子（stdio），确认 API 形状；
  在 `protocol`（或直接复用 rmcp）里定下共享消息/类型封装。*交付：一个能 initialize+tools/list 的 spike + 选型记录。*
- **M1.T2 — upstream(stdio)**：`upstream` crate 连接单个 stdio 上游（`tokio::process` 拉起 npx/uvx）、
  initialize 握手、`tools/list`，映射进 `catalog::ToolDef` 并加 `{server}__` 命名空间。
- **M1.T3 — upstream(HTTP/SSE)**：增加 Streamable HTTP（及 SSE）上游传输。
- **M1.T4 — upstream 生命周期**：健康检查、指数退避重连、**故障隔离**（单上游挂不影响其它）、订阅
  `notifications/tools/list_changed`。
- **M1.T5 — catalog 并发与去重**：索引改为 **build-then-swap（ArcSwap）**，刷新时不阻塞检索（遗留项③）；
  上游工具 `{server}__{name}` **冲突/重复检测**（遗留项④）。
- **M1.T6 — metatools**：实现三个元工具：`search_tools`（接 retrieval）、`get_tool_details`（接 catalog）、
  `call_tool`（接 router）；统一的结构化错误（`isError`）。
- **M1.T7 — router**：命名空间名 → (上游, 原始工具名) 的映射与转发；回传结果/错误；死上游返回可自愈错误。
- **M1.T8 — downstream(stdio)**：`downstream` crate 以 rmcp **server** 暴露三个元工具，走 **stdio**（本地客户端）。
- **M1.T9 — downstream(HTTP) + `serve` 命令**：用 axum 暴露 **Streamable HTTP**；`mcpgw serve` 读取配置启动网关。
- **M1.T10 — config 扩展**：新增 `[server]`、`[[upstream]]` 配置段；**策略白名单单一来源**（遗留项①：
  让 config 校验委托给 retrieval 的"是否实现"判断，避免双份清单漂移）。
- **M1.T11 — 端到端集成测试**：mock 一个 stdio 上游 MCP → mcpgw → 模拟 MCP 客户端跑 search→inspect→execute
  全链路；`list_changed` 触发重建索引的测试；上游崩溃隔离测试。
- **M1.T12 — 真实客户端冒烟矩阵**：Claude Desktop / Cursor / Claude Code 各连一次 mcpgw 验证可用。

**验收**：≥3 个上游聚合、客户端仅见 3 元工具且 `tools/list` 稳定不变；全链路通过；单上游崩溃不影响其它；
`list_changed` 自动重建索引；典型会话工具定义 token 占用相对"全量塞工具"显著下降。

**开工前需敲定的设计问题**：rmcp 具体版本与 server/client API；下游同时支持 stdio+HTTP 还是先 stdio；
`call_tool` 的超时/并发模型；上游凭据透传方式。

---

## M2 — 检索深度（向量 / 混合 / subagent）

**目标**：在已接通的网关上，落地 spec 要求的可插拔多策略，默认 **BM25 + 向量 混合**，云 embedding，后端可配置。

**Tasks**
- **M2.T1 — Python 原型 + golden**：按"脚本先验证再下沉"决策，用脚本在共享 golden 数据上复现 BM25、原型
  向量检索与 RRF 混合，产出/校准期望排序（与 Rust 共用同一份 golden）。
- **M2.T2 — VectorStrategy（云 embedding）**：OpenAI 风格 embedding 客户端；索引期对"工具文本"向量化、
  查询期余弦检索；配置 `provider/model/api_key_env`（密钥仅环境变量引用）。
- **M2.T3 — embedding 缓存/批处理/降级**：缓存与批量请求；embedding API 失败时**自动降级到 BM25**（记 warn）。
- **M2.T4 — HybridStrategy（RRF）**：BM25 + 向量的 Reciprocal Rank Fusion（原计划设为默认；实际改为 **opt-in**，默认仍 bm25）。✅ 已完成（M2-B）
- **M2.T5 — SubagentStrategy（可选）**：用小模型（Haiku/Flash）选工具，behind config。
- **M2.T6 — factory/config 接线 + 质量测试**：把 vector/hybrid/subagent 接入 `build_strategy`，各自 golden/质量测试。
- **M2.T7 —（可选）本地 embedding**：fastembed-rs / ONNX 离线向量化，作为云 API 的替代。

**验收**：配置切换策略生效；hybrid 默认；向量/混合在 golden 上不劣于 BM25；API 失败自动降级；各策略有测试。

---

## M3 — 稳健公网暴露（隧道 / 反代 / OAuth）

**目标**：把 mcpgw 安全地暴露到外网——直击现有工具在反代/OAuth 上踩的坑（参考 MetaMCP 的 issue 集群）。

**Tasks**
- **M3.T1 — 一键隧道**：cloudflared/ngrok 集成助手 + 文档（一条命令把本地网关挂到公网）。
- **M3.T2 — 反代正确性**：修复"反代/TLS 终止后 DCR/`.well-known` 返回 localhost"这类问题；尊重
  `X-Forwarded-*` 或配置的外部 URL。
- **M3.T3 — OAuth（MCP Spec 2025-06-18）**：正确的 state/CSRF 校验、token 交换、DCR；并提供 API-Key 鉴权替代。
- **M3.T4 — TLS 与安全加固**：TLS 指引；密钥经环境变量/vault，绝不写日志。
- **M3.T5 — 暴露集成测试**：在"模拟反代"后做端到端鉴权与发现测试。

**验收**：在反代 + 隧道后，标准 MCP 客户端能完成 OAuth/API-Key 鉴权并正常发现/调用；无 localhost 回灌问题。

---

## M4 — 控制面板（Web + 移动端）

**目标**：顺手的图形界面管理 namespace/server/tool——补上现有工具普遍缺失的移动端体验。

**Tasks**
- **M4.T1 — 管理 HTTP API**：网关上暴露管理 API（列服务/工具、启停、健康、最近调用）。
- **M4.T2 — Web 前端**：React/TS——服务与工具列表、开关、分组、健康状态。
- **M4.T3 — 移动友好 + 分享**：响应式 UI；一键启停；**扫码分享端点**。
- **M4.T4 — 实时调用/用量视图**：对接 M6 的可观测能力。
- **M4.T5 — 面板鉴权**：复用 M3/M5 的鉴权与权限。

**验收**：可在 Web 与手机上启停某个 server/工具、分组、扫码分享、查看调用；改动即时生效。

---

## M5 — 细粒度 RBAC + 危险操作审批

**目标**：按 key/用户/工具的访问控制 + 危险操作的人审拦截——开源侧普遍空白。

**Tasks**
- **M5.T1 — RBAC 模型**：key/用户/角色；按 tool 的 allow/deny；按 namespace。
- **M5.T2 — 策略执行**：在 router/metatools 中按策略**过滤 search 结果**并**拦截 call**。
- **M5.T3 — 危险操作审批**：human-in-the-loop 确认；支持按脚本/会话的"分类授权"（categorical grant）。
- **M5.T4 — 策略管理 UI**：扩展 M4 面板。
- **M5.T5 — 策略决策审计**：记录每次允许/拒绝。

**验收**：不同 key 看到/可调的工具不同；危险工具调用触发审批；策略决策可审计。

---

## M6 — 可观测性 / 审计 + Programmatic Tool Calling

**目标**：全链路调用日志/追踪/用量；以及"code-mode"沙箱内编排工具调用。

**Tasks**
- **M6.T1 — 结构化调用日志 + 追踪**：每次调用（上游、延迟、错误）可追踪。
- **M6.T2 — 用量指标 + 导出**：按 tool/key 统计；Prometheus/OpenTelemetry 导出。
- **M6.T3 — 审计持久化**：调用与决策的审计落库。
- **M6.T4 —（大、可选）Programmatic tool calling / code-mode**：把上游工具 schema 生成为受沙箱保护的类型化
  API，模型写脚本在沙箱执行，只回传结果（按官方 Client Best Practices 的 code-mode）。

**验收**：能定位"哪个上游慢/挂"；有用量统计与审计；code-mode 能在沙箱安全编排多工具调用。

---

## 横切工作（持续进行）

- **CI**：GitHub Actions——`cargo fmt --check` + `clippy -D warnings` + `cargo test`（M1 开始即引入）。
- **打包/发布**：单静态二进制 + Docker 镜像；版本与 changelog。
- **文档（L1–L4 分层）— 强制**：约定与索引见 [`docs/README.md`](../../README.md)。
  L1 模块概览 / L2 各组件职责与接口 / L3 组件细节 / L4 逐源文件 API。
  **每个开发 task 的 Definition of Done 包含：边写代码边填充对应层级文档，并随代码在同一提交里提交。**
  双重审查须把"对应层级文档是否同步更新"作为验收项。M0 的 L1–L4 文档已补齐（见 `docs/`）。
- **安全基线**：密钥仅环境变量/vault、永不入日志；依赖审计（`cargo audit`）。

---

## 推进节奏（建议）

1. 先做 **M1**（让它真正成为网关）——这是价值最大的一跳。
2. 然后 **M2**（检索深度，兑现核心差异化）与 **M3**（公网暴露）按需并行。
3. 之后 **M4 → M5**（面板与权限），**M6** 贯穿后期。
4. 每个里程碑开工：`writing-plans` 生成逐步 TDD 计划 → subagent-driven-development 执行（每 task 双重审查）。
