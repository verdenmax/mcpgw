# L2 — `metatools` 组件

## 职责

网关的**元工具逻辑层**（M1-B.1）：在一份**不可变** `GatewaySnapshot`（catalog + 已建索引的检索策略）之上，
提供三个纯函数式的元工具——`search_tools`（检索）、`get_tool_details`（取详情）、`call_tool`（路由执行）。
它定义快照类型与元工具错误类型，但**不**持有可变状态、**不**做 `ArcSwap` 热替换（那是 `gateway` 的事），也**不**
暴露 MCP server（那是下游服务 M1-B.2 的事）。

## 公开接口

### 类型 `GatewaySnapshot`（`snapshot.rs`）
聚合工具目录 + 其上的检索策略的不可变快照，被 `gateway` 用 `ArcSwap` 持有。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(catalog: Catalog, strategy: Box<dyn RetrievalStrategy>) -> Self` | 由 catalog 与**已 `index` 过**的策略构造；`catalog`/`strategy` 字段对外只读（`pub(crate)`） |
| `catalog` | `(&self) -> &Catalog` | 只读访问聚合工具目录（不暴露检索策略）；供 dashboard 等只读消费者枚举/计数工具 |

### 类型 `ToolSummary`（`snapshot.rs`）
一条 `search_tools` 命中：命名空间化工具名 + 一行描述 + 检索分数。

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | `String` | 命名空间化名 `{server}__{name}` |
| `description` | `String` | 一行描述 |
| `score` | `f32` | 检索相关度分数（越大越相关，命中按其降序）；向后兼容的新增字段，dashboard 发现追踪据此呈现 |

`#[derive(Debug, Clone, PartialEq, serde::Serialize)]`。

### 元工具函数（`tools.rs`）

| 函数 | 签名 | 说明 |
|------|------|------|
| `search_tools` | `async (&GatewaySnapshot, query: &str, top_k: usize) -> Vec<ToolSummary>` | 经策略检索（`strategy.search(...).await`），最多 `top_k` 条，最佳在前；`ScoredTool` → `ToolSummary` |
| `get_tool_details` | `<'a>(&'a GatewaySnapshot, name: &str) -> Option<&'a ToolDef>` | 按命名空间名在 catalog 中查完整定义 |
| `call_tool` | `async (&GatewaySnapshot, &UpstreamRegistry, name: &str, args: Option<Map<String, Value>>) -> Result<CallToolResult, MetaError>` | 经 catalog 查到 `(server, tool)` 后转发到对应上游 |

### 错误 `MetaError`（`error.rs`）
`#[derive(Debug, thiserror::Error)]` 枚举，下游服务（B.2）将其映射为 MCP `isError`：

- `ToolNotFound(String)` — catalog 中无此命名空间名。
- `UpstreamUnavailable(String)` — 该工具的 server 不在注册表中。
- `Timeout` — 上游调用超时（来自 `UpstreamError::Timeout`）。
- `Call(String)` — 其它上游调用失败（携带 `UpstreamError` 文本）。

## 依赖

- 内部：`catalog`（`Catalog` / `ToolDef`）、`retrieval`（`RetrievalStrategy` / `ScoredTool`）、`upstream`
  （`UpstreamRegistry` / `UpstreamHandle` / `UpstreamError` 路由与转发）。
- 外部：`rmcp`（`CallToolResult` 返回类型）、`serde`/`serde_json`、`thiserror`。

## 被谁使用

- `gateway`（M1-B.1）：构造并经 `ArcSwap` 持有 `GatewaySnapshot`，是 `call_tool` 的 `UpstreamRegistry` 提供者。
- 下游 MCP 服务（M1-B.2）：把三个元工具函数包装成对外的 MCP 工具，并把 `MetaError` 映射成 `isError`。

## 关键不变量

- `GatewaySnapshot` **不可变**：一旦 `new`，其 catalog 与策略不再改变；更新靠 `gateway` 整体换新快照。
- **路由经 catalog 查，绝不拆 `__`**：`call_tool`/`get_tool_details` 用命名空间名在 catalog 里查到 `ToolDef`，
  再用其 `server`/`name` 字段路由；原始工具名本身含 `__` 时（如 `srv__weird__tool`）仍正确——朴素拆分会出错。
- 策略须在 `new` 之前 `index` 过，`search_tools` 才有结果。

## 向下导航

- 内部细节见 L3：[metatools](../L3-details/metatools.md)
- 逐文件 API 见 L4：[tools](../L4-api/metatools-tools.md) · [snapshot](../L4-api/metatools-snapshot.md)
