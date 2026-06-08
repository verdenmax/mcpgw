# L4 — `crates/catalog/src/lib.rs` API

源文件：`crates/catalog/src/lib.rs`。本 crate 的全部公开项如下。

## `struct ToolDef`
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub server: String,
    pub name: String,
    pub description: String,
    #[serde(default)] pub input_schema: serde_json::Value,
}
```
一个上游 MCP 工具的目录存储形态。`input_schema` 在 JSON 中缺省为 `Value::Null`。

### `ToolDef::qualified_name`
```rust
pub fn qualified_name(&self) -> String
```
返回 `"{server}__{name}"`。无错误。

## `struct Catalog`
```rust
#[derive(Debug, Default, Clone)]
pub struct Catalog { /* tools: BTreeMap<String, ToolDef>（私有） */ }
```
按 qualified name 键入的内存注册表。

| 方法 | 签名 | 返回 / 说明 |
|------|------|-------------|
| `new` | `pub fn new() -> Self` | 空目录（= `Default`） |
| `upsert` | `pub fn upsert(&mut self, tool: ToolDef)` | 按 `tool.qualified_name()` 插入/替换 |
| `remove_server` | `pub fn remove_server(&mut self, server: &str)` | 删除 `t.server == server` 的全部工具 |
| `get` | `pub fn get(&self, qualified_name: &str) -> Option<&ToolDef>` | 命中则返回引用 |
| `len` | `pub fn len(&self) -> usize` | 工具数 |
| `is_empty` | `pub fn is_empty(&self) -> bool` | 是否为空 |
| `iter` | `pub fn iter(&self) -> impl Iterator<Item = &ToolDef>` | 按 qualified name 升序遍历 |
| `from_tooldefs` | `pub fn from_tooldefs(tools: Vec<ToolDef>) -> Self` | 逐个 upsert 构建 |
| `from_json_str` | `pub fn from_json_str(json: &str) -> Result<Self, CatalogLoadError>` | 解析 JSON 数组；失败返回 `CatalogLoadError` |

## `struct CatalogLoadError`
```rust
#[derive(Debug)]
pub struct CatalogLoadError(pub serde_json::Error);
```
- 实现 `Display`：`"failed to parse catalog JSON: {inner}"`。
- 实现 `std::error::Error`，`source()` 返回内部 `serde_json::Error`。
- 字段 `.0` 公开，可直接取用底层错误（行列、分类等）。

> 详见 L3：[catalog](../L3-details/catalog.md)
