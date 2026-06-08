# L4 — `crates/config/src/lib.rs` API

源文件：`crates/config/src/lib.rs`。

## `struct Config`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)] pub retrieval: RetrievalConfig,
}
```

| 方法 | 签名 | 返回 / 说明 |
|------|------|-------------|
| `from_toml_str` | `pub fn from_toml_str(s: &str) -> Result<Self, ConfigError>` | 解析 + 校验；语法/未知字段 → `Parse`，语义非法 → `Invalid` |
| `default_from_empty` | `pub fn default_from_empty() -> Self` | 全默认配置（解析空串，恒成功） |

> 私有 `fn validate(&self) -> Result<(), ConfigError>`：校验 `strategy ∈ {bm25,vector,hybrid}` 且 `top_k > 0`。

## `struct RetrievalConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    pub strategy: String,   // 默认 "bm25"
    pub top_k: usize,       // 默认 8
}
```
实现 `Default`（`strategy="bm25"`、`top_k=8`）。

## `enum ConfigError`
```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}
```
- `Parse`：TOML 语法错误或未知字段。
- `Invalid`：语义校验失败（未知 strategy、`top_k == 0`）。

> 行为细节见 L3：[config](../L3-details/config.md)
