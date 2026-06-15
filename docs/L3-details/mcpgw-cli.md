# L3 — `mcpgw`（CLI）细节

## `run()` / `main()` 分离

`run(cli) -> Result<(), String>` 承载全部逻辑；`main() -> ExitCode` 仅负责把 `Result` 映射为退出码并把
`error: {e}` 打到 stderr。这种分离让退出码正确且便于测试。`Serve` 子命令在 `run` 内自建多线程 tokio 运行时
`block_on(run_serve(cfg))`（其余子命令是同步的）。

## `serve`：`prepare_state` / `run_serve` 分离

- **`prepare_state(&cfg)`**：纯装配 + 可单测——`GatewayState::new(strategy)` → 建 `mpsc::channel::<String>(64)`
  作 `RebuildTrigger` → `upstream::connect::connect_all(registry, &cfg.upstreams, tx)`（eager-connect、降级启动）
  → 初始 `rebuild_snapshot()` → 返回 `(Arc<GatewayState>, rx)`。两步都 `tracing::info!` 出 connect/rebuild 摘要。
- **`run_serve(cfg)`**：装 stderr tracing subscriber → 校验**至少启用一种传输**（`cfg.server.stdio` 或
  `[server.http].enabled`，均未启用则 `Err("no server transport enabled ...")`）→ **env 密钥启动期解析**
  `resolve_api_keys` + `validate_upstream_http_env`（见下）→ **构造观测 sinks**（以 `observe::TracingSink`
  打底；M6.T1，stdio 与 HTTP 两个 `GatewayServer` 共享同一份 `Arc<[Arc<dyn CallSink>]>`）→ 若
  `cfg.audit.enabled` 则 `observe::spawn_writer(&cfg.audit.path, AUDIT_CHANNEL_CAPACITY).map_err(...)?`
  （**打开文件 fail-fast**）追加一个 `JsonlSink` 进 sinks 并保留 `Option<AuditWriter>` → `prepare_state` →
  `tokio::spawn(run_rebuild_worker(state.clone(), rx))` → 预绑定 HTTP `TcpListener`（仅 http_enabled，bind 失败即
  `Err`）→ **并发关闭模型**（见下）→ **审计 writer 有界优雅 drain**（见下）→ best-effort 逐个 `remove` +
  `Arc::try_unwrap` + `shutdown().await` 上游子进程。

## 审计 writer 的优雅 drain（`drop(sinks)` → 有界 join）

`[audit].enabled` 时，`run_serve` 在 `select!` 返回后先 `drop(sinks)` **释放外层 sinks 切片**——所有
`JsonlSink` clone 全部 drop 后 channel 才断连，writer 线程才会 drain/flush/`sync_all`（fsync）并退出。随后
`if let Some(writer)`：`tokio::time::timeout(AUDIT_DRAIN_TIMEOUT, tokio::task::spawn_blocking(move || writer.join()))`
——把阻塞 `join` 放到 blocking 线程、并加 **5 秒（`AUDIT_DRAIN_TIMEOUT`）兜底**，超时只 `warn!`（不卡死关停）。

- **drop 顺序要求**：stdio 分支的 `GatewayServer` 是 `select!` 分支局部，分支结束即 drop，故 `drop(sinks)`
  能释放最后一份 clone、writer 立即 drain。
- **超时兜底的对象**：HTTP 路径按会话在分离 task 里铸造 per-session `GatewayServer`，**悬挂的 http 连接 sink
  克隆**（含 **idle keep-alive 会话**仍持一份 clone）会让 channel 迟迟不断连——此即 `AUDIT_DRAIN_TIMEOUT` 兜底
  的场景（已入队的记录仍被 writer 按批 flush，与超时无关）。

## 并发关闭模型（`select!` over stdio / http / ctrl_c）

`run_serve` 用 `tokio::select!` 在三个 future 间并发：stdio 分支（`GatewayServer::serve(stdio())` → `waiting()`，
带 `if stdio_enabled` 前置条件）、HTTP 分支（`axum::serve(listener, router)`，带 `if http_enabled`）、以及
`tokio::signal::ctrl_c()`。`select!` 的 `if <precondition>` 为假时**不会构造**对应 `async` future，故
`http_bound.unwrap()` 只在 `http_enabled` 时执行（stdio 同理）。**任一分支完成即返回**，其余 future 被 drop，随后进入
统一收尾关闭。两个下游 server 共享同一个 `Arc<GatewayState>`（HTTP 用 `state.clone()`，stdio 用另一份 clone）。
HTTP listener 在进入 `select!` **之前**预绑定（fail-fast 暴露端口占用/权限错误，而非进入循环后才失败）。

> **HTTP 守护进程模式（无本地 stdio 客户端）**：`[server].stdio` 默认 `true`，此时即使同时启用 HTTP，stdio
> server 的 stdin-EOF（或无 stdin 附着）也会经 `select!` 拆掉整个进程。若要以 HTTP 守护进程方式长期运行，应设
> `[server].stdio = false`，让进程仅由 HTTP server + Ctrl-C 驱动，否则 stdin EOF 会直接关停它。

## env 密钥启动期解析（fail-fast）

- **`resolve_api_keys(&cfg)`**：无 `[server.http]` 或其 `enabled = false` 时返回空 `Vec`（故关闭 HTTP 的配置即便
  含引用未设 env 的 `api_key` 也不会启动失败）；否则逐个 `std::env::var(&k.env)` 读取
  `[[server.http.api_key]]` 的密钥，任一缺失即 `Err`。错误消息仅含 `name`/`env` **名**，**绝不含密钥值**。
- **`validate_upstream_http_env(&cfg)`**：遍历 HTTP 上游，校验 `bearer_env` 与 `headers` 各引用的 env 均已设置；
  缺失即 `Err`（消息仅含上游名/header 名/env 名）。目的：缺凭证时**启动即失败**，而非运行期静默退化成 401 循环。
- 两者均在连接任何上游、绑定任何端口**之前**执行，确保配置/凭证问题以清晰消息尽早暴露。

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
  `--config` 路径（配置文件的 `top_k` 生效；`strategy = "vector"`/`"hybrid"` 离线 `search` 无 embedder 注入，故报 `EmbedderRequired` 非零退出）。
- `main.rs` 单测：`cli_parses_serve_subcommand`（clap 解析 `serve`）；
  `run_serve_builds_initial_snapshot_with_no_upstreams`（空配置下 `prepare_state` 成功、产出可用空快照）；
  `resolve_api_keys_reads_env_and_fails_fast_on_missing`、`resolve_api_keys_empty_when_no_http`、
  `validate_upstream_http_env_fails_fast_on_missing_bearer`（env 密钥启动期解析的 fail-fast 行为）。
- `crates/mcpgw/tests/audit.rs`（集成，M6.T3）：`serve_with_audit_enabled_writes_jsonl_for_a_meta_tool_call`
  —— 起真实 `mcpgw serve --config`（`[audit].enabled`、stdio-only），经 stdio 客户端调一次 `search_tools`、断开
  触发优雅 drain，poll 审计文件后断言首行是 `meta_tool == "search_tools"` / `outcome == "ok"` 的元数据行且
  **不含 payload**（端到端验证装配 + 落盘 + 关停 drain）。

## 真实端到端冒烟 `crates/mcpgw/tests/smoke_real.rs`（`#[ignore]` 门控）

- 黑盒：用 `env!("CARGO_BIN_EXE_mcpgw")` spawn **真实 mcpgw 二进制**，它再把官方参考服务器
  `@modelcontextprotocol/server-everything` 作为真 stdio 上游拉起；rmcp 客户端分别经 **stdio** 与
  **HTTP** 驱动 `list_tools(=3) → search → get_tool_details → call_tool`（含 `everything__echo` 与
  连字符工具 `everything__get-sum`，验证「绝不按 `__` 拆名」路由不变量）。
- 默认被 `#[ignore]` 跳过（需联网/`npx`/Node）。显式运行：
  `cargo test -p mcpgw --test smoke_real -- --ignored --nocapture`。
- **运维要点（真实踩坑）**：基于 `npx`/`uvx`/`node` 的 stdio 上游，其 `env_passthrough` **必须包含
  `PATH`（通常还要 `HOME`）**——mcpgw 会 `env_clear()` 子进程环境、仅回注白名单变量，缺 `PATH` 则
  `npx`/`node` 无法解析自身依赖、上游连接失败被降级 skip。

## 向量检索验证脚本 `scripts/embed_check.py`（stdlib-only）

- 仅用 Python 标准库（`json`/`math`/`os`/`sys`/`urllib`），不引入第三方依赖；对照工具目录手动核验真实
  OpenAI 兼容 `/embeddings` 端点的语义排序——「在信任 Rust 集成前先用脚本验证」的路线图步骤。
- 读取环境：`OPENAI_API_KEY`（必填，缺失则提示并退出）、`MCPGW_EMBED_BASE_URL`（默认 OpenAI）、
  `MCPGW_EMBED_MODEL`（默认 `text-embedding-3-small`）。
- 运行（有真实 key 时）：`OPENAI_API_KEY=sk-... python scripts/embed_check.py`；对每个语义 query 打印
  各工具的余弦相似度排名，预期 top-1 与直觉一致。无 key 则脚本自动跳过。

## 门控真实向量冒烟 `crates/mcpgw/tests/smoke_vector_real.rs`（`#[ignore]` 门控）

- 用真实 OpenAI 兼容端点嵌入工具目录，断言一条**语义** query（与任何工具描述无共享字面 token）仍能将
  正确工具排在首位——这是 BM25 无法做到的语义增益（例：`"communicate with my team"` → 首位应为
  `slack__post_message`）。
- 默认被 `#[ignore]` 跳过（需联网 + `OPENAI_API_KEY`，可选 `MCPGW_EMBED_BASE_URL`/`MCPGW_EMBED_MODEL`）；
  普通 `cargo test` 不触网。显式运行：
  `cargo test -p mcpgw --test smoke_vector_real -- --ignored --nocapture`。

## 相关

- 接口见 L2：[mcpgw-cli](../L2-components/mcpgw-cli.md)；逐文件 API 见 L4：[mcpgw/main.rs](../L4-api/mcpgw-main.md)
