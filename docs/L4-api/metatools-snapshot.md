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

### `GatewaySnapshot::catalog`
```rust
pub fn catalog(&self) -> &Catalog
```
对聚合工具目录的**只读**访问器（如供 dashboard 的 `/api/*` 只读地枚举/计数工具）。不暴露内部检索策略、不可变。

## `struct ToolSummary`
```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    pub score: f32,
}
```
一条 `search_tools` 命中：

- `name` — 命名空间化工具名 `{server}__{name}`。
- `description` — 一行描述。
- `score` — 检索相关度分数（越大越相关；命中按 `score` 降序返回）。来自 `RetrievalStrategy` 的 `SearchHit.score`，
  **向后兼容的新增字段**（dashboard 的发现追踪/搜索视图用它呈现命中分数）。

`Serialize` 用于下游服务把搜索结果序列化为 MCP 工具响应。

> 详见 L3：[metatools](../L3-details/metatools.md)
