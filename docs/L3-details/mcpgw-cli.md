# L3 — `mcpgw`（CLI）细节

## `run()` / `main()` 分离

`run(cli) -> Result<(), String>` 承载全部逻辑；`main() -> ExitCode` 仅负责把 `Result` 映射为退出码并把
`error: {e}` 打到 stderr。这种分离让退出码正确且便于测试。`Serve` 子命令在 `run` 内自建多线程 tokio 运行时
`block_on(run_serve(cfg))`（其余子命令是同步的）。

## `serve`：`prepare_state` / `run_serve` 分离

- **`prepare_state(&cfg)`**：纯装配 + 可单测——`GatewayState::new(strategy)` → 建 `mpsc::channel::<String>(64)`
  作 `RebuildTrigger` → `upstream::connect::connect_all(registry, &cfg.upstreams, tx)`（eager-connect、降级启动）
  → 初始 `rebuild_snapshot()` → 返回 `(Arc<GatewayState>, rx)`。两步都 `tracing::info!` 出 connect/rebuild 摘要。
- **`run_serve(cfg)`**：装 stderr tracing subscriber → 校验 `cfg.server.stdio`（否则 `Err`）→ `prepare_state`
  → `tokio::spawn(run_rebuild_worker(state.clone(), rx))` → `GatewayServer::new(state, cfg.retrieval.top_k)
  .serve(stdio())` → `service.waiting().await`（阻塞到 transport 关闭）→ best-effort 逐个 `remove` +
  `Arc::try_unwrap` + `shutdown().await` 上游子进程。

**日志走 stderr**：`tracing_subscriber::fmt().with_writer(std::io::stderr)`（`try_init`，已初始化则忽略），
默认 `EnvFilter` `info`（可经 `RUST_LOG` 覆盖）。stdout 必须留给 MCP 协议帧，故日志严格走 stderr。

## 错误处理风格

薄 CLI，统一用 `map_err(|e| e.to_string())` 把各类错误压成 `String`（含 `GatewayError`、`UpstreamError`、rmcp
serve 错误）。I/O 错误保留了上下文（`read catalog {path}: {e}`）。对当前规模足够；若 CLI 长大，可换 `anyhow`。

## JSON 输出与 `unwrap` 安全性

- `get-details`：序列化 `&ToolDef`（字段为 `String`/`Value`，恒可序列化）。
- `search`：序列化由 `serde_json::json!` 构造的 `Vec<Value>`。即便 `score: f32` 为 NaN/Inf 也**不会
  panic**——`json!` 经 `Number::from_f64`，非有限值得到 `Value::Null` 而非不可序列化节点。
- 因此两处 `to_string_pretty(...).unwrap()` 均经验证安全。

## `top_k` 优先级

`top_k.unwrap_or(cfg.retrieval.top_k)` —— 命令行 `--top-k` 优先，否则用配置默认。

## 已知点

- `--catalog` 默认值 `tests/fixtures/tools.json` 是 CWD 相对、指向**测试 fixture**，仅当从工作区根目录
  运行时才解析得到。属开发便利；面向用户发布前应改为必填或 env/config 驱动（见路线图遗留项）。

## 集成测试 `crates/mcpgw/tests/cli.rs` + 单测（`main.rs`）

- `cli.rs` 通过 `env!("CARGO_BIN_EXE_mcpgw")` 调用**真实编译出的二进制**（非库捷径）。
- fixture 经 `CARGO_MANIFEST_DIR` + `../../tests/fixtures/tools.json` 解析，与 CWD 无关。
- 覆盖：search 输出为 JSON 且 `--top-k 1` 实际限为 1 条；get-details 成功；未知工具失败（非零退出）；
  `--config` 路径（配置文件的 `top_k` 生效；`strategy = "vector"` 经二进制冒出 `NotImplemented` 非零退出）。
- `main.rs` 单测：`cli_parses_serve_subcommand`（clap 解析 `serve`）；
  `run_serve_builds_initial_snapshot_with_no_upstreams`（空配置下 `prepare_state` 成功、产出可用空快照）。

## 相关

- 接口见 L2：[mcpgw-cli](../L2-components/mcpgw-cli.md)；逐文件 API 见 L4：[mcpgw/main.rs](../L4-api/mcpgw-main.md)
