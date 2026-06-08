# L2 — `catalog` 组件

## 职责

工具数据模型 + 跨上游服务器的**命名空间工具注册表** + 从 JSON 加载。是检索索引与（未来）调用路由
的唯一真相源。不了解检索、配置或 CLI。

## 公开接口

### 类型 `ToolDef`
一个上游 MCP 工具在目录中的存储形态。

| 字段 | 类型 | 说明 |
|------|------|------|
| `server` | `String` | 上游服务器命名空间（如 `"github"`） |
| `name` | `String` | 该服务器内的原始工具名（如 `"create_issue"`） |
| `description` | `String` | 一行描述，供检索与 `search_tools` 输出 |
| `input_schema` | `serde_json::Value` | 完整 JSON 输入 schema，供 `get_tool_details`（缺省 `Null`） |

- `ToolDef::qualified_name(&self) -> String` — 命名空间化的全局唯一标识：`{server}__{name}`。
- 派生 `Debug, Clone, PartialEq, Serialize, Deserialize`。

### 类型 `Catalog`
按 qualified name 键入的内存注册表（`BTreeMap` 支撑，迭代顺序确定）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `() -> Catalog` | 空目录 |
| `upsert` | `(&mut self, ToolDef)` | 按 qualified name 插入/替换 |
| `remove_server` | `(&mut self, &str)` | 删除某 server 的全部工具 |
| `get` | `(&self, &str) -> Option<&ToolDef>` | 按 qualified name 查 |
| `len` / `is_empty` | `(&self) -> usize` / `bool` | 计数 |
| `iter` | `(&self) -> impl Iterator<Item=&ToolDef>` | 确定性顺序遍历 |
| `from_tooldefs` | `(Vec<ToolDef>) -> Catalog` | 从列表构建 |
| `from_json_str` | `(&str) -> Result<Catalog, CatalogLoadError>` | 从 JSON 数组解析 |

### 错误 `CatalogLoadError`
`pub struct CatalogLoadError(pub serde_json::Error)`，手写 `Display`/`Error`（含 `source()`），
**不依赖 `thiserror`**（保持 catalog 依赖最小）。

## 依赖

- 外部：`serde`、`serde_json`。
- 内部：无（不依赖任何兄弟 crate）。

## 被谁使用

- `retrieval`：`Bm25Strategy::index` 遍历 `catalog.iter()`。
- `mcpgw`：加载目录、`get` 工具详情。

## 关键不变量

- 主键恒为 `qualified_name()`；`upsert` 同名替换而非重复。
- `BTreeMap` 保证遍历顺序确定 → 检索索引与结果可复现（golden 测试依赖此点）。

## 向下导航

- 内部细节见 L3：[catalog](../L3-details/catalog.md)
- 逐文件 API 见 L4：[catalog/lib.rs](../L4-api/catalog-lib.md)
