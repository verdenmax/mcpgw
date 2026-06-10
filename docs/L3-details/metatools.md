# L3 — `metatools` 细节

## `GatewaySnapshot` 的不可变性

`GatewaySnapshot { catalog: Catalog, strategy: Box<dyn RetrievalStrategy> }` 两字段均 `pub(crate)`，仅经
`new(catalog, strategy)` 一次性装配，之后不再 mutate。元工具函数全部接 `&GatewaySnapshot`（只读）。这让 `gateway`
可以把它放进 `ArcSwap` 并以"整体换新"的方式更新——快照内部无需任何锁。约定：传入的 `strategy` 必须**已对该 catalog
`index` 过**，否则 `search_tools` 返回空。

## `search_tools`：`ScoredTool` → `ToolSummary` 映射

`search_tools(snap, query, top_k)` 把检索委托给策略，再投影结果：

```text
snap.strategy.search(query, top_k)  ->  Vec<ScoredTool { qualified_name, description, score }>
        每条 map 成 ToolSummary { name: qualified_name, description }
```

- `score` 字段被**丢弃**——元工具只对外暴露名字与描述，排序信息不外泄。
- 顺序保持策略给的"最佳在前"。`top_k` 由策略负责截断。

## `get_tool_details`：按命名空间名查 catalog

`get_tool_details(snap, name) -> Option<&ToolDef>` 直接 `snap.catalog.get(name)`，返回对快照内 `ToolDef` 的借用
（生命周期绑定 `&snap`）。无命中返回 `None`。

## `call_tool`：catalog 路由（绝不拆 `__`）

```text
1. let def = snap.catalog.get(name)            -> 无则 Err(MetaError::ToolNotFound(name))
2. let handle = registry.get(&def.server)      -> 无则 Err(MetaError::UpstreamUnavailable(def.server))
3. handle.call_tool(&def.name, arguments).await -> 映射 UpstreamError
```

**关键**：`server` 与原始 `name` 来自 catalog 中存好的 `ToolDef` 字段，**不是**对命名空间名 split `"__"` 得来。

- **反例**：上游工具原名本身含 `__`，如 server `srv` 的工具 `weird__tool`，命名空间名为 `srv__weird__tool`。
  朴素地按首个/末个 `__` 拆分都会把 `(server, tool)` 切错；而 catalog 路由用存储的 `server="srv"` /
  `name="weird__tool"` 永远正确（单测 `get_tool_details_handles_tool_names_containing_double_underscore` 验证）。

## `MetaError` → MCP `isError`（B.2 关系）

`MetaError` 是元工具层的领域错误，下游 MCP 服务（M1-B.2）把它映射为工具响应的 `isError: true`：

| `MetaError` | 触发点 | 含义 |
|-------------|--------|------|
| `ToolNotFound(name)` | catalog 查不到 | 客户端给了未知工具名 |
| `UpstreamUnavailable(server)` | registry 无该 server | 该工具所属上游未连接/已摘除 |
| `Timeout` | 上游调用超时 | 由 `UpstreamError::Timeout` 转来 |
| `Call(msg)` | 其它上游失败 | 携带底层 `UpstreamError` 文本 |

## 超时来自 `UpstreamHandle`

`metatools` 自身**不**计时。超时由 `upstream` 层的 `UpstreamHandle` 用其 `call_timeout`（`with_call_timeout`
设定，默认 30s）在 `call_tool` 内 `tokio::time::timeout` 施加；触发时返回 `UpstreamError::Timeout { server }`，
`call_tool` 再把它收敛为无 server 信息的 `MetaError::Timeout`，其余 `UpstreamError` 收敛为 `MetaError::Call`。

## 测试覆盖

- `search_tools_returns_namespaced_summaries`：命中名为命名空间名、描述非空。
- `get_tool_details_returns_full_def_or_none`：命中返回完整 `ToolDef`，未命中 `None`。
- `get_tool_details_handles_tool_names_containing_double_underscore`：原名含 `__` 仍可按命名空间名取回，
  且 `server`/`name` 正确（朴素 split 会错）。

## 相关

- 接口见 L2：[metatools](../L2-components/metatools.md)
- 逐文件 API 见 L4：[tools](../L4-api/metatools-tools.md) · [snapshot](../L4-api/metatools-snapshot.md)
- 路由依赖的上游层见：[upstream L3](./upstream.md)
