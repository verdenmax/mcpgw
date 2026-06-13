# L4 — `crates/mcpgw/src/main.rs` API

源文件：`crates/mcpgw/src/main.rs`。这是**二进制 crate**，没有 `pub` 库 API；这里记录其内部项与
对外的命令行接口。

## 内部项

| 项 | 签名 | 说明 |
|----|------|------|
| `struct Cli` | clap `Parser` | 全局 `--catalog: PathBuf`（默认 `tests/fixtures/tools.json`）、`--config: Option<PathBuf>`、`command: Command` |
| `enum Command` | clap `Subcommand` | `Search { query: String, top_k: Option<usize> }`、`GetDetails { name: String }`、`Serve` |
| `fn load_config` | `(&Option<PathBuf>) -> Result<Config, String>` | `None` → `Config::default_from_empty()`；`Some(p)` → 读文件 + `from_toml_str` |
| `fn load_catalog` | `(&Path) -> Result<Catalog, String>` | 读 catalog JSON + `Catalog::from_json_str`；**仅** `search`/`get-details` 调用 |
| `fn run` | `(Cli) -> Result<(), String>` | 主逻辑：加载配置 → 执行子命令（`Serve` 起 tokio 运行时 `block_on(run_serve)`） |
| `fn build_embedder` | `(&Config) -> Result<Option<Arc<dyn retrieval::Embedder>>, String>` | 按 `retrieval.strategy` 建 embedder：`"vector"` → 从 `api_key_env` **启动期 fail-fast** 读 key（缺失即 `Err`，消息仅含 env **名**不含值）、建 `OpenAiEmbedder` 再裹 `CachingEmbedder`（缓存跨 snapshot 重建复用）；其它（`bm25`）→ `None`；单测可达 |
| `async fn prepare_state` | `(&Config) -> Result<(Arc<GatewayState>, mpsc::Receiver<String>), String>` | `build_embedder` → 有则 `GatewayState::with_embedder` 否则 `GatewayState::new` → `connect_all`（eager-connect 上游、装 trigger 发送端）→ 初始 `rebuild_snapshot` → 返回状态 + worker 接收端；单测可达 |
| `fn resolve_api_keys` | `(&Config) -> Result<Vec<String>, String>` | 解析 `[[server.http.api_key]]` 各密钥的 env 值；无 `[server.http]` 时返回空 `Vec`；任一 env 缺失即 `Err`（消息仅含 `name`/`env` **名**，不含密钥值）；单测可达 |
| `fn validate_upstream_http_env` | `(&Config) -> Result<(), String>` | 校验所有 HTTP 上游引用的 env（`bearer_env` + `headers` 各值）均已设置；缺失即 fail-fast `Err`（消息仅含上游名/header 名/env 名，不含值）；单测可达 |
| `async fn run_serve` | `(Config) -> Result<(), String>` | 装 stderr tracing → 校验至少启用一种传输（`stdio` 或 `http.enabled`，否则 `Err`）→ fail-fast 解析 `resolve_api_keys` + `validate_upstream_http_env` → `prepare_state` → spawn `run_rebuild_worker` → 预绑定 HTTP listener（`TcpListener::bind`，bind 失败即 `Err`）→ `tokio::select!` 并发跑 stdio（`GatewayServer::serve(stdio())` → `waiting()`）/ HTTP（`axum::serve`）/ `ctrl_c`，任一完成即统一关闭 → 收尾 best-effort `shutdown` 各上游 |
| `fn main` | `() -> ExitCode` | 解析 CLI、调 `run`，映射退出码（成功 0 / 失败 1，错误打 stderr） |

## 命令行接口（对外契约）

### `mcpgw search <query> [--top-k N]`
检索 `query`，打印 `[{ "name", "description", "score" }]` 的美化 JSON 数组。`--top-k` 覆盖配置 `top_k`。

### `mcpgw get-details <name>`
打印 qualified name 为 `<name>` 的工具完整 JSON；不存在则 stderr 报 `error: no such tool: <name>` 并以
退出码 1 结束。

### `mcpgw serve`
起**活的 MCP 网关**，按配置并发跑 **stdio 与/或 HTTP** 两个下游 server，共享同一个 `Arc<GatewayState>`：
`prepare_state`（`connect_all` eager-connect 上游 → 初始 `rebuild_snapshot`）→ spawn `run_rebuild_worker`
（处理上游 `tools/list_changed`）→ `tokio::select!` over {stdio `waiting()`、`axum::serve`、`ctrl_c`} 统一关闭，
任一分支完成即触发整体关闭、drop 其余 future → best-effort `shutdown` 各上游。**日志走 stderr**（stdout 留给 MCP
协议帧）。启动期 **fail-fast** 顺序：先校验至少启用一种传输（`cfg.server.stdio` 或 `[server.http].enabled`，
均未启用则 `Err`），再 `resolve_api_keys` + `validate_upstream_http_env` 解析/校验所有 env 引用的密钥（缺失即中止，
消息仅含字段/env 名），再 `prepare_state`，最后**预绑定** HTTP `TcpListener`（bind 失败即 `Err`）后才进入 `select!`。
此模式**不**读 `--catalog`（catalog 来自上游）。

### 全局
- `--catalog <path>`：工具目录 JSON（默认指向测试 fixture，见 L3 说明）。**仅** `search`/`get-details` 使用。
- `--config <path>`：可选 TOML 配置；省略则用全默认。

> 行为与测试细节见 L3：[mcpgw-cli](../L3-details/mcpgw-cli.md)
