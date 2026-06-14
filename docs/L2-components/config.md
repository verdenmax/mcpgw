# L2 — `config` 组件

## 职责

解析并校验 mcpgw 的 TOML 配置。含 `[retrieval]`、`[[upstream]]`（M1-A，stdio + M1-C 的 http）与 `[server]`
（M1-B，含 M1-C 的 `[server.http]`）三段。不了解检索内部，也不反向依赖 `retrieval`。

## 公开接口

### 类型 `Config`
顶层配置。`#[serde(deny_unknown_fields)]`；字段 `retrieval: RetrievalConfig`、`upstreams: Vec<UpstreamConfig>`
（`rename = "upstream"`）、`server: ServerConfig`（均 `#[serde(default)]`）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `from_toml_str` | `(&str) -> Result<Config, ConfigError>` | 解析 + 校验 |
| `default_from_empty` | `() -> Config` | 全默认配置（解析空串） |

### 类型 `RetrievalConfig`
`[retrieval]` 段。`#[serde(default, deny_unknown_fields)]`。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `strategy` | `String` | `"bm25"` | `bm25` \| `vector` \| `hybrid` \| `subagent`（默认 bm25；其余经 config opt-in） |
| `top_k` | `usize` | `8` | `search_tools` 返回条数 |
| `vector` | `Option<VectorConfig>` | `None` | `[retrieval.vector]` 段；`strategy ∈ {vector,hybrid}` 时必填 |
| `subagent` | `Option<SubagentConfig>` | `None` | `[retrieval.subagent]` 段；`strategy == "subagent"` 时必填 |

### 类型 `VectorConfig`
`[retrieval.vector]` 段。`#[serde(deny_unknown_fields)]`：OpenAI 兼容 embedding 提供方。密钥**只经 env 变量名引用**。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `base_url` | `String` | `"https://api.openai.com/v1"` | embedding endpoint 基址 |
| `model` | `String` | （必填） | embedding 模型名 |
| `api_key_env` | `String` | （必填） | 持有 API key 的 env 变量名（不含明文） |
| `dim` | `Option<usize>` | `None` | 期望向量维度（可选，传给 provider） |
| `timeout_ms` | `Option<u64>` | `None` | 单次请求超时（毫秒） |
| `batch_size` | `Option<usize>` | `None` | **预留 / 未启用**：当前不做分块，所有输入一次性请求（保留给 M2-B） |

### 类型 `SubagentConfig`
`[retrieval.subagent]` 段。`#[serde(deny_unknown_fields)]`：OpenAI 兼容 chat 提供方，给 subagent 重排器用。密钥**只经 env 变量名引用**。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `base_url` | `String` | `"https://api.openai.com/v1"` | chat completions endpoint 基址 |
| `model` | `String` | （必填） | chat 模型名（建议小模型，如 Haiku/Flash/gpt-4o-mini） |
| `api_key_env` | `String` | （必填） | 持有 API key 的 env 变量名（不含明文） |
| `timeout_ms` | `Option<u64>` | `None` | 单次请求超时（毫秒） |
| `candidates` | `Option<usize>` | `None` | BM25 预筛 shortlist 大小；`None` → retrieval 默认（`DEFAULT_CANDIDATES = 20`）；`validate()` 拒绝 `Some(0)` |

### 类型 `UpstreamConfig` / `UpstreamTransport`
`[[upstream]]` 数组。每项含 `name`（命名空间前缀，非空白、禁含 `__`）、`call_timeout_ms`（默认 `30_000`）、
经 `#[serde(flatten)]` 摊平的内部标签枚举 `UpstreamTransport`：
- `Stdio { command, args, env_passthrough }`（`transport = "stdio"`）。`env_passthrough` 是传给子进程的环境变量名
  **allow-list**——`upstream::connect` 先清空子进程环境再仅注入这些（且存在于 mcpgw 环境的）变量；默认子进程拿不到
  父环境，须显式列出（如 `PATH`/凭据）。
- `Http { url, bearer_env, headers }`（`transport = "http"`，M1-C 新增）：远程 Streamable HTTP MCP 上游。
  `bearer_env` 是持有 **原始 bearer token** 的 env 变量名（可选；rmcp 在线路上自动加 `Authorization: Bearer ` 前缀，
  配置/token 本身**不含**前缀）；`headers` 是「头名 → 持有该头值的 env 变量名」的**内联表**
  （`headers = { "X-Api-Version" = "REMOTE_VER" }`，刻意用内联表而非 `[[upstream.header]]` 数组表以适配
  `flatten` + 内部标签枚举）。所有认证值**只经 env 引用**，配置里不出现任何明文密钥。

### 类型 `ServerConfig` / `HttpConfig` / `ApiKeyConfig`
`[server]` 段。`#[serde(default, deny_unknown_fields)]`。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `stdio` | `bool` | `true` | 是否把 3 个元工具经 stdio MCP server 暴露。HTTP 守护进程模式（无本地 stdio 客户端）应设为 `false`，否则 stdin EOF 会经 `select!` 关停整个进程，仅由 HTTP server + Ctrl-C 驱动 |
| `http` | `Option<HttpConfig>` | `None` | `[server.http]` 段；省略 → HTTP 关闭（M1-C） |

`[server.http]`（`HttpConfig`，`#[serde(default, deny_unknown_fields)]`）：Streamable HTTP server 设置。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `false` | 须显式 opt-in 才启动 HTTP server |
| `bind` | `String` | `"127.0.0.1:8970"` | 监听地址；公网暴露请配合隧道/反向代理（M3） |
| `path` | `String` | `"/mcp"` | MCP 端点挂载路径 |
| `api_keys` | `Vec<ApiKeyConfig>` | `[]` | `#[serde(rename = "api_key")]` → `[[server.http.api_key]]`；为空 = 不鉴权（依赖 localhost 绑定） |

`[[server.http.api_key]]`（`ApiKeyConfig`，`#[serde(deny_unknown_fields)]`）：每项 `name`（仅作日志/可观测标识，
**绝不打印 key 值**）+ `env`（持有 key 明文的 env 变量名）。密钥明文只经 env 引用，配置里只存 env 变量名。

### 错误 `ConfigError`
`enum ConfigError { Parse(toml::de::Error), Invalid(String) }`（`thiserror`，`Parse` 带 `#[from]`）。
- `Parse`：TOML 语法错误或未知字段（`deny_unknown_fields`）。
- `Invalid`：语义校验失败（未知 strategy；`top_k == 0`；`strategy ∈ {vector,hybrid}` 缺 `[retrieval.vector]` 段或其 `base_url`/`model`/`api_key_env` 空白；`strategy == "subagent"` 缺 `[retrieval.subagent]` 段或其 `base_url`/`model`/`api_key_env` 空白或 `candidates == Some(0)`；upstream `name` 空白/含 `__`/重复；http 上游 `url` 空白）。

## 依赖

- 外部：`serde`、`toml`、`thiserror`。
- 内部：无。

## 被谁使用

- `mcpgw`：`load_config` 读取/默认配置；`search`/`get-details` 用 `cfg.retrieval`，`serve` 用 `cfg.upstreams`
  （eager-connect）、`cfg.server.stdio` 与 `cfg.server.http`（并发选择 stdio/HTTP 传输、解析 API-Key env）、
  `cfg.retrieval.top_k`（下游默认 top_k）；`serve` 还按 `cfg.retrieval.strategy` 经 `build_backends` 装配检索后端
  （`vector`/`hybrid` → `cfg.retrieval.vector` 建 `OpenAiEmbedder → CachingEmbedder`；`subagent` →
  `cfg.retrieval.subagent` 建 `OpenAiChat` + `candidates`）并注入 `GatewayState::with_backends`（启动期 fail-fast
  读 `api_key_env`）。
  **密钥/头值的 env 引用在 `serve` 启动时 fail-fast 解析**（缺失即报错、
  仅含字段/env 名），config 自身只校验结构、不读取 env 值。
- `upstream::connect`：读 `UpstreamConfig` / `UpstreamTransport` 起 stdio 子进程上游或连接 http 上游。

## 关键不变量

- 校验在 `from_toml_str` 内联完成、且 `validate` 私有 → 调用方无法跳过校验。
- `default_from_empty` 的 `expect` 安全：空串总是产出合法默认配置（见 L3）。

## 向下导航

- 内部细节见 L3：[config](../L3-details/config.md)
- 逐文件 API 见 L4：[config/lib.rs](../L4-api/config-lib.md)
