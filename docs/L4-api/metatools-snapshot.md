# L4 — `crates/metatools/src/snapshot.rs` API

源文件：`crates/metatools/src/snapshot.rs`。不可变网关快照 + `search_tools` 结果项。

## `struct GatewaySnapshot`
```rust
pub struct GatewaySnapshot {
    pub(crate) catalog: Catalog,
    pub(crate) strategy: Box<dyn RetrievalStrategy>,
}
```
聚合工具目录 + 其上已建索引的检索策略的不可变快照。两字段 `pub(crate)`，对外只读。由 `gateway` 经 `ArcSwap` 持有。

### `GatewaySnapshot::new`
```rust
pub fn new(catalog: Catalog, strategy: Box<dyn RetrievalStrategy>) -> Self
```
由 catalog 与一个**已 `index` 过**的策略构造快照。无错误。约定：`strategy` 须已对 `catalog` 建索引，否则
`search_tools` 返回空。

## `struct ToolSummary`
```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
}
```
一条 `search_tools` 命中：

- `name` — 命名空间化工具名 `{server}__{name}`。
- `description` — 一行描述。

`Serialize` 用于下游服务把搜索结果序列化为 MCP 工具响应。

> 详见 L3：[metatools](../L3-details/metatools.md)
