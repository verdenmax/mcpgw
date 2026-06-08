# L2 — `mcpgw`（CLI 二进制）组件

## 职责

唯一的集成者：把 `catalog` + `config` + `retrieval` 装配成一个开发者面向的 CLI，用于在接入真正的 MCP
I/O 层（M1）之前验证检索内核。

## 命令行接口（即本 crate 的对外"接口"）

二进制名 `mcpgw`，clap 派生。

### 全局参数
| 参数 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `--catalog` | 路径 | `tests/fixtures/tools.json` | 工具目录 JSON（数组） |
| `--config` | 路径（可选） | 无 → 全默认 | TOML 配置文件 |

### 子命令
- `search <query> [--top-k N]` — 自然语言检索；打印 `[{name, description, score}]` 的美化 JSON 数组。
  `--top-k` 覆盖配置的 `top_k`。
- `get-details <name>` — 按 qualified name 打印某工具的完整 JSON；不存在则报错并以非零码退出。

### 退出码
`0` 成功；`1` 失败（错误信息打到 stderr，前缀 `error:`）。

## 依赖

- 内部：`catalog`、`retrieval`、`config`。
- 外部：`clap`（derive）、`serde_json`。

## 行为流水

`run()`：读取并解析 catalog → 加载配置 → 按子命令：`search` 用 `build_strategy` + `index` + `search`
并输出 JSON；`get-details` 用 `catalog.get`。`main()` 把 `Result` 映射为 `ExitCode`。

## 向下导航

- 内部细节见 L3：[mcpgw-cli](../L3-details/mcpgw-cli.md)
- 逐文件 API 见 L4：[mcpgw/main.rs](../L4-api/mcpgw-main.md)
