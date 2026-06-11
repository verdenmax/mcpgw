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
- `serve` — 起活的 stdio MCP 网关：eager-connect 上游 → 初始重建快照 → list_changed 重建 worker →
  `GatewayServer` over stdio 暴露 3 个元工具。日志走 **stderr**（stdout 留给 MCP 协议）。

### 退出码
`0` 成功；`1` 失败（错误信息打到 stderr，前缀 `error:`）。

## 依赖

- 内部：`catalog`、`retrieval`、`config`、`gateway`、`upstream`、`downstream`（`serve` 用）。
- 外部：`clap`（derive）、`serde_json`、`tokio`（`serve` 运行时）、`rmcp`（stdio transport）、
  `tracing-subscriber`（stderr 日志）。

## 行为流水

`run()`：加载配置 → 按子命令分派。`search`/`get-details` 读 `--catalog` 后用 `build_strategy` + `index` +
`search` 或 `catalog.get`；`serve` 起 tokio 运行时 `block_on(run_serve)`——`prepare_state`（`connect_all` →
初始 `rebuild_snapshot`）→ spawn `run_rebuild_worker` → `GatewayServer::serve(stdio())` → `waiting()` →
收尾 `shutdown` 上游。`main()` 把 `Result` 映射为 `ExitCode`。

## 向下导航

- 内部细节见 L3：[mcpgw-cli](../L3-details/mcpgw-cli.md)
- 逐文件 API 见 L4：[mcpgw/main.rs](../L4-api/mcpgw-main.md)
