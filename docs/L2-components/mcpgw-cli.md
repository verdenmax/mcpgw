# L2 — `mcpgw`（CLI 二进制）组件

## 职责

唯一的集成者：把检索内核（`catalog` + `config` + `retrieval`）装配成开发者 CLI（`search` / `get-details`），
并在 M1-B.2 起把活网关（`upstream` + `gateway` + `downstream`）装配成 `serve` 子命令——即 mcpgw 的运行态入口。

## 命令行接口（即本 crate 的对外"接口"）

二进制名 `mcpgw`，clap 派生。

### 全局参数
| 参数 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `--catalog` | 路径 | `tests/fixtures/tools.json` | 工具目录 JSON（数组），**仅** `search`/`get-details` 使用 |
| `--config` | 路径（可选） | 无 → 全默认 | TOML 配置文件 |

### 子命令
- `search <query> [--top-k N]` — 自然语言检索；打印 `[{name, description, score}]` 的美化 JSON 数组。
  `--top-k` 覆盖配置的 `top_k`。
- `get-details <name>` — 按 qualified name 打印某工具的完整 JSON；不存在则报错并以非零码退出。
- `serve` — 起活的 MCP 网关，按配置**并发**跑 stdio 与/或 HTTP 两个下游 server（共享一个 `GatewayState`）：
  eager-connect 上游 → 初始重建快照 → list_changed 重建 worker → `GatewayServer` 暴露 3 个元工具，
  通过 `tokio::select!` over {stdio / HTTP / ctrl_c} 统一关闭。启动期 fail-fast 解析所有 env 密钥（缺失即中止）。
  装配处构造一份默认观测 sinks `[observe::TracingSink]`（M6.T1），注入 stdio 与 HTTP 两个 `GatewayServer` 共享，
  每次元工具调用产出仅元数据的 `observe::CallRecord` 并 fan-out。
  至少需启用一种传输（`[server].stdio` 或 `[server.http].enabled`）。日志走 **stderr**（stdout 留给 MCP 协议）。

### 退出码
`0` 成功；`1` 失败（错误信息打到 stderr，前缀 `error:`）。

## 依赖

- 内部：`catalog`、`retrieval`、`config`、`gateway`、`upstream`、`downstream`、`observe`（`serve` 装配默认 `[TracingSink]`）。
- 外部：`clap`（derive）、`serde_json`、`tokio`（`serve` 运行时，`net`/`signal` 用于 HTTP listener 与 ctrl_c）、
  `rmcp`（stdio transport）、`axum`（HTTP `serve`）、`tracing-subscriber`（stderr 日志）。

## 行为流水

`run()`：加载配置 → 按子命令分派。`search`/`get-details` 读 `--catalog` 后用 `build_strategy` + `index` +
`search` 或 `catalog.get`；`serve` 起 tokio 运行时 `block_on(run_serve)`——fail-fast 解析 env 密钥 →
构造默认观测 sinks `[observe::TracingSink]` → `prepare_state`（`connect_all` → 初始 `rebuild_snapshot`）→
spawn `run_rebuild_worker` → `tokio::select!`
并发跑 stdio（`serve(stdio())` → `waiting()`）/ HTTP（`axum::serve`）/ `ctrl_c` → 收尾 `shutdown` 上游。
`main()` 把 `Result` 映射为 `ExitCode`。

## 向下导航

- 内部细节见 L3：[mcpgw-cli](../L3-details/mcpgw-cli.md)
- 逐文件 API 见 L4：[mcpgw/main.rs](../L4-api/mcpgw-main.md)
- 观测装配（`serve` 注入的 sinks）见 L2：[observe](./observe.md)
