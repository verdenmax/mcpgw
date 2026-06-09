# L4 — `crates/config/src/lib.rs` API

源文件：`crates/config/src/lib.rs`。

## `struct Config`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)] pub retrieval: RetrievalConfig,
    #[serde(default, rename = "upstream")] pub upstreams: Vec<UpstreamConfig>,
}
```
`[[upstream]]` 数组通过 `rename = "upstream"` 映射到 `upstreams` 字段（缺省为空）。

| 方法 | 签名 | 返回 / 说明 |
|------|------|-------------|
| `from_toml_str` | `pub fn from_toml_str(s: &str) -> Result<Self, ConfigError>` | 解析 + 校验；语法/未知字段 → `Parse`，语义非法 → `Invalid` |
| `default_from_empty` | `pub fn default_from_empty() -> Self` | 全默认配置（解析空串，恒成功） |

> 私有 `fn validate(&self) -> Result<(), ConfigError>`：校验 `strategy ∈ {bm25,vector,hybrid}`、
> `top_k > 0`，以及每个 upstream 的 `name` 非空白、不含 `__`、不重复（否则 `Invalid`）。

## `struct UpstreamConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]   // 注意：无 deny_unknown_fields（因 flatten）
pub struct UpstreamConfig {
    pub name: String,                                   // 命名空间前缀；非空白、禁止含 "__"
    #[serde(default = "default_call_timeout_ms")]
    pub call_timeout_ms: u64,                           // 默认 30_000
    #[serde(flatten)] pub transport: UpstreamTransport,
}
```

## `enum UpstreamTransport`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum UpstreamTransport {
    Stdio {
        command: String,
        #[serde(default)] args: Vec<String>,
        #[serde(default)] env_passthrough: Vec<String>,
    },
}
```
内部标签字段 `transport`（值如 `"stdio"`）与变体字段在同一 TOML 表中（经 `#[serde(flatten)]`）。
M0/M1-A 仅实现 `stdio`；`http` 等留待 M1-C。

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
- `Invalid`：语义校验失败（未知 strategy、`top_k == 0`、upstream `name` 空白/含 `__`/重复）。

> 行为细节见 L3：[config](../L3-details/config.md)
