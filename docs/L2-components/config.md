# L2 — `config` 组件

## 职责

解析并校验 mcpgw 的 TOML 配置。M0 仅含 `[retrieval]` 段（`[server]` / `[[upstream]]` 留待 M1）。
不了解检索内部，也不反向依赖 `retrieval`。

## 公开接口

### 类型 `Config`
顶层配置。`#[serde(deny_unknown_fields)]`；字段 `retrieval: RetrievalConfig`（`#[serde(default)]`）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `from_toml_str` | `(&str) -> Result<Config, ConfigError>` | 解析 + 校验 |
| `default_from_empty` | `() -> Config` | 全默认配置（解析空串） |

### 类型 `RetrievalConfig`
`[retrieval]` 段。`#[serde(default, deny_unknown_fields)]`。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `strategy` | `String` | `"bm25"` | `bm25` \| `vector` \| `hybrid`（M0 仅实现 bm25） |
| `top_k` | `usize` | `8` | `search_tools` 返回条数 |

### 错误 `ConfigError`
`enum ConfigError { Parse(toml::de::Error), Invalid(String) }`（`thiserror`，`Parse` 带 `#[from]`）。
- `Parse`：TOML 语法错误或未知字段（`deny_unknown_fields`）。
- `Invalid`：语义校验失败（未知 strategy；`top_k == 0`）。

## 依赖

- 外部：`serde`、`toml`、`thiserror`。
- 内部：无。

## 被谁使用

- `mcpgw`：`load_config` 读取/默认配置；把 `cfg.retrieval.strategy` / `top_k` 传给检索层。

## 关键不变量

- 校验在 `from_toml_str` 内联完成、且 `validate` 私有 → 调用方无法跳过校验。
- `default_from_empty` 的 `expect` 安全：空串总是产出合法默认配置（见 L3）。

## 向下导航

- 内部细节见 L3：[config](../L3-details/config.md)
- 逐文件 API 见 L4：[config/lib.rs](../L4-api/config-lib.md)
