# L4 — `crates/config/src/lib.rs` API

源文件：`crates/config/src/lib.rs`。

## `struct Config`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)] pub retrieval: RetrievalConfig,
    #[serde(default, rename = "upstream")] pub upstreams: Vec<UpstreamConfig>,
    #[serde(default)] pub server: ServerConfig,
    #[serde(default)] pub audit: AuditConfig,
}
```
`[[upstream]]` 数组通过 `rename = "upstream"` 映射到 `upstreams` 字段（缺省为空）；`[server]` 段缺省取
`ServerConfig::default()`；`[audit]` 段缺省取 `AuditConfig::default()`（即审计关闭）。

| 方法 | 签名 | 返回 / 说明 |
|------|------|-------------|
| `from_toml_str` | `pub fn from_toml_str(s: &str) -> Result<Self, ConfigError>` | 解析 + 校验；语法/未知字段 → `Parse`，语义非法 → `Invalid` |
| `default_from_empty` | `pub fn default_from_empty() -> Self` | 全默认配置（解析空串，恒成功） |

> 私有 `fn validate(&self) -> Result<(), ConfigError>`：校验 `strategy ∈ {bm25,vector,hybrid,subagent}`、
> `top_k > 0`、`strategy ∈ {vector,hybrid}` 时必须有 `[retrieval.vector]` 段且其 `base_url`/`model`/`api_key_env`
> 非空白、`strategy == "subagent"` 时必须有 `[retrieval.subagent]` 段且其 `base_url`/`model`/`api_key_env` 非空白且
> `candidates != Some(0)`，以及每个 upstream 的 `name` 非空白、不含 `__`、**不以 `_` 开头或结尾**（边界下划线会重新拼出 `__` 分隔符）、不重复，
> 且 `[server.http].path`（若有）必须以 `/` 开头且长于 `/`（拒绝 `""`/`"/"`/无前导斜杠），
> 且**不得含通配/参数段**（即不含 `{`、`}`、`*`，如 `/{id}`、`/{*rest}`、`/a*b`）——否则 `Invalid`。
> 后者同样在启动期、axum 之前校验：含这类段的路径会让 axum 在 `nest_service` 构建路由时 panic（`/{*rest}`），
> 或把 MCP 静默挂到动态捕获段（`/{id}`）上。

## `struct UpstreamConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]   // 注意：无 deny_unknown_fields（因 flatten）
pub struct UpstreamConfig {
    pub name: String,                                   // 命名空间前缀；非空白、禁止含 "__"、禁止以 "_" 开头或结尾
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
        #[serde(default)] env_passthrough: Vec<String>,   // 环境变量 allow-list（见下）
    },
    Http {                                                 // M1-C 新增：远端 Streamable HTTP 上游
        url: String,                                       // 端点 URL，如 "https://example.com/mcp"
        #[serde(default)] bearer_env: Option<String>,      // 持有 bearer token 的 env 变量名
        #[serde(default)] headers: HashMap<String, String>,// 头名 → 持有该头值的 env 变量名
    },
}
```
内部标签字段 `transport`（值如 `"stdio"` / `"http"`）与变体字段在同一 TOML 表中（经 `#[serde(flatten)]`）。
M0/M1-A 仅实现 `stdio`；M1-C 加入 `http` 变体（连接逻辑在 T2+，T1 仅扩 schema）。

`Http` 变体的认证值**只经 env 引用**：`bearer_env` 是持有 bearer token 的环境变量名（发为
`Authorization: Bearer <token>`）；`headers` 是「头名 → 持有该头值的 env 变量名」的**内联表**
（`headers = { "X-Api-Version" = "REMOTE_VER" }`），不在配置里出现任何明文密钥。`url` 经 `validate()`
强制非空白（否则 `Invalid`）。

`env_passthrough` 是传给子进程的环境变量名 **allow-list**：`upstream::connect` 启动子进程时先 `env_clear()`
清空子进程环境，再仅把这些变量（且存在于 mcpgw 自身环境时）传入。子进程默认**拿不到**父进程环境，须显式列出
（如 `PATH`/`HOME`/凭据变量）。

## `struct ServerConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub stdio: bool,                // 默认 true
    pub http: Option<HttpConfig>,   // 默认 None（省略 [server.http] -> HTTP 关闭）
}
```
`[server]` 段：选择下游对外的 transport。`stdio = true`（默认）表示把 3 个元工具经 stdio MCP server 暴露；
`[server.http]` 段（M1-C）提供可选的 Streamable HTTP server。无 `flatten`，故 `deny_unknown_fields` 生效
（`[server]` 内未知键 → `Parse`）。实现 `Default`（`stdio = true`、`http = None`）。

## `struct HttpConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    pub enabled: bool,                  // 默认 false（须显式开启）
    pub bind: String,                   // 默认 "127.0.0.1:8970"
    pub path: String,                   // 默认 "/mcp"；validate() 要求以 "/" 开头、长于 "/"，且不含 {}/* 通配段
    #[serde(rename = "api_key")]
    pub api_keys: Vec<ApiKeyConfig>,    // 对应 [[server.http.api_key]]，默认空
}
```
`[server.http]` 段：Streamable HTTP server 设置。`enabled` 默认 `false`（须 opt-in）；`bind` 默认绑定
localhost（对外暴露请用 tunnel/反向代理）；`path` 是 MCP 端点挂载路径。`api_keys` 经 `#[serde(rename = "api_key")]`
映射 `[[server.http.api_key]]` 数组，为空表示无鉴权（依赖 localhost 绑定）。无 `flatten`，故 `deny_unknown_fields`
生效。实现 `Default`（`enabled=false`、`bind="127.0.0.1:8970"`、`path="/mcp"`、`api_keys=[]`）。

## `struct ApiKeyConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyConfig {
    pub name: String,   // 日志/可观测性标签，**绝不**是密钥值本身
    pub env: String,    // 持有密钥的 env 变量名
}
```
一个被接受的 API key：密钥**只经 env 变量名引用**（`env`），配置里不出现明文密钥；`name` 仅用于
日志/可观测性。

## `struct AuditConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditConfig {
    pub enabled: bool,   // 默认 false（须显式开启）
    pub path: String,    // 默认 "mcpgw-audit.jsonl"
}
```
`[audit]` 段（M6.T3）：可选的**仅追加 JSONL 审计日志**（每次元工具调用一行**仅元数据** `CallRecord`）。
`enabled` 默认 `false`——**省略整个 `[audit]` 段 = 审计关闭**（经容器级 `#[serde(default)]` 取
`AuditConfig::default()`）。`path` 是审计文件路径（create+append；默认 `"mcpgw-audit.jsonl"`，CWD 相对）；
**每个网关进程需各自独立的 path**（同文件多进程并发写是误配，见 L3）。无 flatten，故
`#[serde(default, deny_unknown_fields)]` 生效（段内未知键 → `Parse`）；实现 `Default`（`enabled=false`、
`path="mcpgw-audit.jsonl"`）。`validate()` **不**校验 `path`（落盘失败在 `serve` 启动期由
`observe::spawn_writer` 经 fail-fast 暴露）。

## `struct RetrievalConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    pub strategy: String,             // 默认 "bm25"
    pub top_k: usize,                 // 默认 8
    pub vector: Option<VectorConfig>, // 默认 None；strategy ∈ {vector,hybrid} 时必填
    pub subagent: Option<SubagentConfig>, // 默认 None；strategy == "subagent" 时必填
}
```
实现 `Default`（`strategy="bm25"`、`top_k=8`、`vector=None`、`subagent=None`）。

## `struct VectorConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorConfig {
    #[serde(default = "default_vector_base_url")]
    pub base_url: String,             // 默认 "https://api.openai.com/v1"
    pub model: String,                // 必填
    pub api_key_env: String,          // 必填；持有 key 的 env 变量名（只引用名，不含明文）
    #[serde(default)] pub dim: Option<usize>,
    #[serde(default)] pub timeout_ms: Option<u64>,
    #[serde(default)] pub batch_size: Option<usize>, // 预留/未启用：当前不分块（M2-B）
}
```
`[retrieval.vector]`：OpenAI 兼容 embedding 提供方。密钥**只经 env 变量名引用**（`api_key_env`），
配置里不出现明文。无 flatten，故 `deny_unknown_fields` 生效（未知字段 → `Parse`）。`batch_size`
为**预留字段、当前未启用**：`OpenAiEmbedder` 一次请求发送全部输入，不做分块（留待 M2-B）。

## `struct SubagentConfig`
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentConfig {
    #[serde(default = "default_subagent_base_url")]
    pub base_url: String,             // 默认 "https://api.openai.com/v1"
    pub model: String,                // 必填
    pub api_key_env: String,          // 必填；持有 key 的 env 变量名（只引用名，不含明文）
    #[serde(default)] pub timeout_ms: Option<u64>,
    #[serde(default)] pub candidates: Option<usize>, // BM25 预筛 shortlist 大小；None → retrieval 默认
}
```
`[retrieval.subagent]`：OpenAI 兼容 chat 提供方，给 subagent 重排器用。密钥**只经 env 变量名引用**
（`api_key_env`），配置里不出现明文。无 flatten，故 `deny_unknown_fields` 生效（未知字段 → `Parse`）。
`candidates` 是交给小模型的 BM25 预筛 shortlist 大小，`None` 时取 `retrieval` 的 `DEFAULT_CANDIDATES`；
`validate()` 拒绝 `candidates == Some(0)`。**生产建议设 `timeout_ms`**：subagent 在**每次** `search_tools` 都同步调一次
chat，而 reqwest 无默认超时——挂死的端点会阻塞该次请求（仅影响单次检索，读快照仍无锁）；超时（或任何错误）都会透明降级回 BM25。

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
- `Invalid`：语义校验失败（未知 strategy、`top_k == 0`、`strategy ∈ {vector,hybrid}` 缺 `[retrieval.vector]` 段或其字段空白、`strategy == "subagent"` 缺 `[retrieval.subagent]` 段或其 `base_url`/`model`/`api_key_env` 空白或 `candidates == Some(0)`、upstream `name` 空白/含 `__`/以 `_` 开头或结尾/重复、`[server.http].path` 不以 `/` 开头或仅为 `/`）。

> 行为细节见 L3：[config](../L3-details/config.md)
