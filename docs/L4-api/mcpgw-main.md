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
| `async fn prepare_state` | `(&Config) -> Result<(Arc<GatewayState>, mpsc::Receiver<String>), String>` | 建 `GatewayState` → `connect_all`（eager-connect 上游、装 trigger 发送端）→ 初始 `rebuild_snapshot` → 返回状态 + worker 接收端；单测可达 |
| `async fn run_serve` | `(Config) -> Result<(), String>` | 装 stderr tracing → 校验 `cfg.server.stdio` → `prepare_state` → spawn `run_rebuild_worker` → `GatewayServer::serve(stdio())` → `waiting()` → 收尾 best-effort `shutdown` 各上游 |
| `fn main` | `() -> ExitCode` | 解析 CLI、调 `run`，映射退出码（成功 0 / 失败 1，错误打 stderr） |

## 命令行接口（对外契约）

### `mcpgw search <query> [--top-k N]`
检索 `query`，打印 `[{ "name", "description", "score" }]` 的美化 JSON 数组。`--top-k` 覆盖配置 `top_k`。

### `mcpgw get-details <name>`
打印 qualified name 为 `<name>` 的工具完整 JSON；不存在则 stderr 报 `error: no such tool: <name>` 并以
退出码 1 结束。

### `mcpgw serve`
起**活的 MCP 网关**（stdio）：`prepare_state`（`connect_all` eager-connect 上游 → 初始 `rebuild_snapshot`）→
spawn `run_rebuild_worker`（处理上游 `tools/list_changed`）→ `downstream::GatewayServer` over stdio，运行到
transport 关闭后 best-effort `shutdown` 各上游。**日志走 stderr**（stdout 留给 MCP 协议帧）。
`cfg.server.stdio = false` 时返回错误（HTTP 留待 M1-C）。此模式**不**读 `--catalog`（catalog 来自上游）。

### 全局
- `--catalog <path>`：工具目录 JSON（默认指向测试 fixture，见 L3 说明）。**仅** `search`/`get-details` 使用。
- `--config <path>`：可选 TOML 配置；省略则用全默认。

> 行为与测试细节见 L3：[mcpgw-cli](../L3-details/mcpgw-cli.md)
