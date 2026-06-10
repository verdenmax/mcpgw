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
- **L2**：[catalog](./L2-components/catalog.md) · [retrieval](./L2-components/retrieval.md) · [config](./L2-components/config.md) · [mcpgw-cli](./L2-components/mcpgw-cli.md) · [upstream](./L2-components/upstream.md) · [metatools](./L2-components/metatools.md) · [gateway](./L2-components/gateway.md)
- **L3**：[catalog](./L3-details/catalog.md) · [retrieval](./L3-details/retrieval.md) · [config](./L3-details/config.md) · [mcpgw-cli](./L3-details/mcpgw-cli.md) · [upstream](./L3-details/upstream.md) · [metatools](./L3-details/metatools.md) · [gateway](./L3-details/gateway.md)
- **L4**：[catalog/lib.rs](./L4-api/catalog-lib.md) · [retrieval/lib.rs](./L4-api/retrieval-lib.md) · [config/lib.rs](./L4-api/config-lib.md) · [mcpgw/main.rs](./L4-api/mcpgw-main.md) · [upstream/mapping.rs](./L4-api/upstream-mapping.md) · [upstream/connection.rs](./L4-api/upstream-connection.md) · [upstream/registry.rs](./L4-api/upstream-registry.md) · [metatools/tools.rs](./L4-api/metatools-tools.md) · [metatools/snapshot.rs](./L4-api/metatools-snapshot.md) · [gateway/lib.rs](./L4-api/gateway-lib.md)

> 当前文档覆盖 **M0（检索核心 / Plan 1）**、**M1-A（`upstream` 上游 I/O 层）** 与 **M1-B.1（`metatools` 元工具
> 逻辑 + `gateway` 快照状态/重建）**。下游 MCP 服务与 eager-connect（M1-B.2）及后续里程碑将按上述规则继续补充各层文档。
> 里程碑路线图见 `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`。
