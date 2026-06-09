# L4 — `crates/upstream/src/mapping.rs` API

源文件：`crates/upstream/src/mapping.rs`。把 rmcp 工具映射为命名空间化的 `catalog::ToolDef`，并带重复检测地
摄取进 `Catalog`。

## `fn tool_to_def`
```rust
pub fn tool_to_def(server: &str, tool: &rmcp::model::Tool) -> catalog::ToolDef
```
把单个上游 `Tool`（归于命名空间 `server`）转换为 `ToolDef`。

- **参数**：`server` 命名空间名；`tool` rmcp 工具引用。
- **返回**：`ToolDef { server, name, description, input_schema }`。
- **语义**：
  - `name = tool.name.to_string()`（原始名，未命名空间化；`qualified_name()` 由 catalog 拼成 `{server}__{name}`）。
  - `description = tool.description.as_deref().unwrap_or("")`——`None` → 空串。
  - `input_schema = Value::Object((*tool.input_schema).clone())`——解引用 `Arc<JsonObject>` 并克隆。
- 无错误、不分配额外失败路径。

## `fn ingest_tools`
```rust
pub fn ingest_tools(catalog: &mut catalog::Catalog, server: &str, tools: &[rmcp::model::Tool]) -> usize
```
把某 server 的工具批量摄取进 `catalog`。

- **参数**：`catalog` 摄取目标；`server` 命名空间名；`tools` 待摄取工具切片。
- **返回**：被跳过的 **intra-server 重复工具名**数量（已通过 `tracing::warn!` 告警）。
- **语义**：
  - 用 `HashSet` 去重，**first-dupe-wins**：首次见到的名 `upsert`，同名再现 → warn + skip + 计数。
  - 去重是**每次调用内**（intra-server）的；重复摄取一个已在 catalog 中的 server 会经 `upsert` 覆盖既有条目，
    返回计数**仅**反映 `tools` 内部的碰撞，不与既有 catalog 状态比较。

> 详见 L3：[upstream](../L3-details/upstream.md)
