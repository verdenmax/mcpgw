# L3 — `catalog` 细节

## 命名空间方案

全局标识为 `qualified_name() = "{server}__{name}"`，用**双下划线**分隔。原因：MCP 工具名常含单下划线
（如 `create_issue`），双下划线降低与原名冲突的概率。

- **已知边界**：若 server 名本身含 `__`，理论上仍可能与某 `{name}` 拼接歧义。M0 的目录来自受信任的
  fixture，未做强校验。M1 从真实上游摄取时需加入 server 名约束/冲突检测（见路线图遗留项）。

## 数据结构选择：`BTreeMap<String, ToolDef>`

- 以 qualified name 为键，天然去重（同名 upsert 即替换）。
- `BTreeMap` 的有序遍历使 `iter()` **顺序确定**，从而：
  - BM25 索引构建顺序确定；
  - 同分时的 tie-break 配合 → 检索结果完全可复现（golden 测试依赖）。
- 目录规模小（数十~数百工具），`BTreeMap` 的 O(log n) 完全够用。

## 操作语义

- `upsert`：键由 `tool.qualified_name()` 派生，避免"调用方传错 key"导致的键值漂移；同键覆盖。
- `remove_server`：`retain(|_, t| t.server != server)` —— 基于结构化的 `server` 字段，而非对 key 做前缀
  匹配，避免 `"git"` 误伤 `"github__..."` 这类前缀 bug。
- `from_json_str`：`serde_json::from_str::<Vec<ToolDef>>` → `from_tooldefs`。`input_schema` 借助
  `ToolDef` 上的 `#[serde(default)]`，缺省为 `Value::Null`。

## 错误设计

`CatalogLoadError` 为手写 newtype（包裹 `serde_json::Error`），而非 `thiserror`：
- 目的是让 `catalog` 的依赖保持在 `serde` + `serde_json`，不引入 `thiserror`。
- 实现了 `source()`，返回内部 `serde_json::Error`，使下游错误链（如 `anyhow`）可向下追溯。

## 静默去重（已知限制）

两条 `{server}__{name}` 相同的 JSON 条目会被 `upsert` 静默"最后写入者胜"，无告警。M0 可接受；M1 从
多个真实上游摄取工具时，需要显式的重复/冲突检测（warn 或 error）。

## 测试覆盖

- `qualified_name_joins_server_and_name_with_double_underscore`
- `catalog_upsert_get_and_remove_server`（计数、查找、同名替换、按 server 删除）
- `from_json_str_parses_array_of_tools`（含 `input_schema` 缺省为 Null）
- `from_json_str_rejects_invalid_json`

## 相关

- 接口见 L2：[catalog](../L2-components/catalog.md)；逐文件 API 见 L4：[catalog/lib.rs](../L4-api/catalog-lib.md)
