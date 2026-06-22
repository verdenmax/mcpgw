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
| `fn build_backends` | `(&Config) -> Result<retrieval::Backends, String>` | 按 `retrieval.strategy` 建后端：`"vector"`/`"hybrid"` → embedder（`OpenAiEmbedder` 再裹 `CachingEmbedder`，缓存跨 snapshot 重建复用）；`"subagent"` → chat（`OpenAiChat`）+ `subagent_candidates`；`"bm25"` → 空 `Backends`。所需 key 从对应 `api_key_env` **启动期 fail-fast** 读取（缺失即 `Err`，消息仅含 env **名**不含值）；单测可达 |
| `async fn prepare_state` | `(&Config) -> Result<(Arc<GatewayState>, mpsc::Receiver<String>), String>` | `build_backends` → `GatewayState::with_backends(&cfg.retrieval.strategy, backends)` → `connect_all`（eager-connect 上游、装 trigger 发送端）→ 初始 `rebuild_snapshot` → 返回状态 + worker 接收端；单测可达 |
| `fn resolve_api_keys` | `(&Config) -> Result<Vec<String>, String>` | 解析 `[[server.http.api_key]]` 各密钥的 env 值；无 `[server.http]` 时返回空 `Vec`；任一 env 缺失即 `Err`（消息仅含 `name`/`env` **名**，不含密钥值）；单测可达 |
| `fn validate_upstream_http_env` | `(&Config) -> Result<(), String>` | 校验所有 HTTP 上游引用的 env（`bearer_env` + `headers` 各值）均已设置；缺失即 fail-fast `Err`（消息仅含上游名/header 名/env 名，不含值）；单测可达 |
| `async fn run_serve` | `(Config) -> Result<(), String>` | 装 stderr tracing → 校验至少启用一种传输（`stdio` 或 `http.enabled`，否则 `Err`）→ fail-fast 解析 `resolve_api_keys` + `validate_upstream_http_env` → `prepare_state` → spawn `run_rebuild_worker` → 装配观测 sinks（以 `observe::TracingSink` 打底；`Arc<[Arc<dyn observe::CallSink>]>`，stdio/http 共享）→ `cfg.audit.enabled` 时 `observe::spawn_writer(&cfg.audit.path, observe::AUDIT_CHANNEL_CAPACITY).map_err(...)?` 追加 `JsonlSink`、留 `Option<AuditWriter>`（打开文件 fail-fast）→ **`cfg.dashboard.enabled` 时**追加 `dashboard::MetricsSink` 进 sinks 切片、并建 `dashboard::CallRingSink::new(call_buffer)` 放进 `content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>`（否则空切片）+ 读 `payload_max_bytes`（逐条调用**内容**通道，args/result 只入此环，喂 dashboard `AppState.calls`）；**且 `trace_queries` 时**经 `dashboard::DiscoveryRingSink::spawn(trace_buffer, trace_path)` 建发现追踪 ring + 可选 JSONL writer，装进 `discovery_sinks: Arc<[Arc<dyn observe::DiscoverySink>]>`（否则空切片）→ 预绑定 HTTP listener（`TcpListener::bind`，bind 失败即 `Err`）→ **预绑定 dashboard listener**（同样 fail-fast；非 loopback 且无 auth 时 `warn!`）→ **把 HTTP 起为带 `with_graceful_shutdown`（oneshot 驱动）的后台 task**（`build_router(..., sinks, discovery_sinks, content_sinks, payload_max_bytes)` → `axum::serve`）→ **把 dashboard 起为另一个独立 port 上的后台 task**（构造 `AppState` → `build_dashboard_router(app_state, enforce_loopback_host)` → `axum::serve` + 自己的 `with_graceful_shutdown`）→ `tokio::select!` 等首个关停触发：stdio（`GatewayServer::new(state, top_k, sinks, discovery_sinks, content_sinks, payload_max_bytes)` → `serve(stdio())` → `waiting()`）/ HTTP task / dashboard task **自行结束** / `ctrl_c` → 收尾顺序：`http_shutdown_tx.send(())` → 有界 drain HTTP → `dash_shutdown_tx.send(())` → 有界 drain dashboard（`DASHBOARD_SHUTDOWN_TIMEOUT`，释放其 `AppState` 内 `DiscoveryRingSink` clone）→ `drop(sinks)` 触发审计 channel 断连 → 有界 drain 审计 writer → `drop(discovery_sinks)` + `drop(discovery_ring)` 触发发现 channel 断连 → 有界 drain 发现 writer → best-effort 拆卸上游 |
| `fn transport_str` | `(&UpstreamTransport) -> String` | 把上游 transport 变体映为短标签（`Stdio→"stdio"`、`Http→"http"`），供 dashboard 的上游列表 `UpstreamInfo.transport` 用 |
| `fn unauthenticated_public_bind` | `(bind: &str, has_keys: bool) -> bool` | 判断某 bind 地址是否「无鉴权且非回环」（用于 HTTP/dashboard 的公网暴露 `warn!`，并取反得 dashboard 的 `enforce_loopback_host`）：有 key 直接 `false`；否则解析 `SocketAddr` 判 `!is_loopback`，解析失败时只把字面 `localhost` 当安全 |
| `const AUDIT_DRAIN_TIMEOUT` | `Duration = Duration::from_secs(5)` | 关停时等待审计 writer（及发现 writer）drain+flush+fsync 的上限；超时只 `warn!`（不卡死关停） |
| `const HTTP_SHUTDOWN_TIMEOUT` | `Duration = Duration::from_secs(5)` | 关停时等待 HTTP server 优雅 drain 在途请求 / 关闭 keep-alive 会话的上限；超时只 `warn!`（不卡死关停） |
| `const DASHBOARD_SHUTDOWN_TIMEOUT` | `Duration = Duration::from_secs(3)` | 关停时等待 dashboard server 优雅 drain 的上限；超时只 `warn!`。先于 `drop(sinks)`/发现 writer drain，以便其 `AppState` 内 `DiscoveryRingSink` clone 提前释放 |
| `fn main` | `() -> ExitCode` | 解析 CLI、调 `run`，映射退出码（成功 0 / 失败 1，错误打 stderr） |

## 构建脚本（`crates/mcpgw/build.rs`）

Cargo 自动检测 crate 根的 `build.rs`（无需改 `Cargo.toml`），在**编译期**把版本/构建信息写进 env 供 `main.rs` 的
`env!()` 解析：

| env 变量 | 来源 | 降级 |
|----------|------|------|
| `MCPGW_GIT_SHA` | `git rev-parse --short HEAD` 的 stdout（trim） | 非 git 仓库 / 无 `git` / 命令失败 / 空输出 → `"unknown"` |
| `MCPGW_BUILD_TIME` | `SystemTime::now()` 距 UNIX_EPOCH 的 epoch **秒**（字符串） | 取时失败 → `0` |

两个 `cargo:rustc-env=…` **总是**输出（带降级回退），故 `env!("MCPGW_GIT_SHA")`/`env!("MCPGW_BUILD_TIME")` 在
`main.rs` 恒可解析、不会编译失败。脚本**未**声明 `cargo:rerun-if-*`，故不强制每次重建——`MCPGW_BUILD_TIME` 是
「最近一次让 build.rs 重跑的构建」的近似时间而非每次编译的精确时刻（无密钥/敏感值，仅 SHA + 时间戳）。

## 命令行接口（对外契约）

### `mcpgw search <query> [--top-k N]`
检索 `query`，打印 `[{ "name", "description", "score" }]` 的美化 JSON 数组。`--top-k` 覆盖配置 `top_k`。

### `mcpgw get-details <name>`
打印 qualified name 为 `<name>` 的工具完整 JSON；不存在则 stderr 报 `error: no such tool: <name>` 并以
退出码 1 结束。

### `mcpgw serve`
起**活的 MCP 网关**，按配置并发跑 **stdio 与/或 HTTP** 两个下游 server，共享同一个 `Arc<GatewayState>`：
`prepare_state`（`connect_all` eager-connect 上游 → 初始 `rebuild_snapshot`）→ spawn `run_rebuild_worker`
（处理上游 `tools/list_changed`）→ 装配观测 sinks（以 `observe::TracingSink` 打底；`Arc<[Arc<dyn observe::CallSink>]>`，
stdio 与 HTTP **共享同一切片**，故两条传输的每次元工具调用都被记为一条仅元数据的 `CallRecord` 并写成
结构化 `tool_call` tracing 事件）→ **`[audit].enabled` 时**经 `observe::spawn_writer` 追加 `JsonlSink`
（每次调用再多落一行 JSONL 审计；**打开文件 fail-fast**，开不了即拒绝启动）→ **把 HTTP 起为带
`with_graceful_shutdown`（由 `oneshot` 驱动）的后台 task** → `tokio::select!` 等首个关停触发 over {stdio
`waiting()`、HTTP task **自行结束**（`axum::serve` 出错，置 `http_self_terminated`）、`ctrl_c`}，任一触发即进入
**固定顺序收尾**：`http_shutdown_tx.send(())` 信号 HTTP 优雅关停 → 若 HTTP 未自行结束则 `tokio::time::timeout(
HTTP_SHUTDOWN_TIMEOUT, http_task)` 有界 drain（关闭 keep-alive 会话、释放其 sink clone；超时仅 `warn!`）→
**`drop(sinks)`** 释放 sinks 切片最后一份 clone、触发审计 channel 断连 → `tokio::time::timeout(AUDIT_DRAIN_TIMEOUT,
spawn_blocking(writer.join()))` **有界优雅 drain** 审计 writer（drain+flush+fsync；超时仅 `warn!`）。**因 HTTP 会话先
释放 sink，审计 drain 现在迅速完成、fsync 确实跑到，不再总是等满 5 秒** → best-effort `remove` 各上游并按
`Arc::try_unwrap` 二分拆卸：**独占** → `shutdown().await`、**共享** → `cancel()`（经 rmcp cancellation token
fire-and-forget 取消，共享 handle 不再被静默跳过）。**日志走 stderr**（stdout 留给 MCP 协议帧）。启动期 **fail-fast** 顺序：先校验至少启用一种传输
（`cfg.server.stdio` 或 `[server.http].enabled`，均未启用则 `Err`），再 `resolve_api_keys` +
`validate_upstream_http_env` 解析/校验所有 env 引用的密钥（缺失即中止，消息仅含字段/env 名），再 `prepare_state`，
再据 `[audit]` 装配 `JsonlSink`（打不开审计文件即 `Err`），最后**预绑定** HTTP `TcpListener`（bind 失败即 `Err`）
后才进入 `select!`。此模式**不**读 `--catalog`（catalog 来自上游）。

#### 只读 dashboard（`[dashboard].enabled`）
当 `[dashboard].enabled` 时，`serve` 额外把**只读可视化面板**起为**独立 task、独立 port**（默认
`127.0.0.1:8971`、**localhost、无鉴权**——非 loopback 绑定会 `warn!`）：把 `dashboard::MetricsSink` 追加进
观测 sinks 切片（与 tracing/审计同走 `CallRecord` 扇出，仅元数据）；**另**建 `dashboard::CallRingSink::new(call_buffer)`
放进**独立**的 `content_sinks`（`CallContentSink`）通道 + 读 `payload_max_bytes`，喂 `AppState.calls`——逐条调用的
**args/result 内容只入此内存环**，绝不进 `CallRecord` 元数据/审计扇出（见 [downstream L3 调用内容捕获](../L3-details/downstream.md)）；
**且 `trace_queries` 时**经
`DiscoveryRingSink::spawn` 建发现追踪 ring（+ 可选 `trace_path` JSONL writer），把 `discovery_sinks` 注入
**两条**下游（stdio + HTTP），使 `search_tools` 的 query/命中走**独立 opt-in** 通道。dashboard listener
**先于** spawn 任何 serve task **预绑定**（fail-fast，对称于 HTTP）。`serve` 据绑定地址算
`enforce_loopback_host = !unauthenticated_public_bind(&cfg.dashboard.bind, false)`（绑 loopback → `true`）并传给
`build_dashboard_router`：绑 loopback 时挂 `require_local_host` 中间件，把 `Host` 非本地的请求 `403`（**抗 DNS
重绑定、非鉴权**）；绑非 loopback（已 `warn` 的显式暴露）则跳过。构造 `AppState` 时另把
`dashboard::AboutInfo::from_config(&cfg, dashboard::VersionInfo { version: CARGO_PKG_VERSION, git_sha: MCPGW_GIT_SHA,
build_time: MCPGW_BUILD_TIME })` 填进 `AppState.about`（启动时组装一次、只读、仅非敏感配置/限额 + 版本，喂
`/api/about`；`git_sha`/`build_time` 来自上面的 `build.rs`）。关停时按固定顺序：HTTP drain →
**dashboard drain（`DASHBOARD_SHUTDOWN_TIMEOUT = 3s`，先释放其 `AppState` 内 `DiscoveryRingSink` clone）** →
`drop(sinks)` + 审计 drain → `drop(discovery_sinks/ring)` + 发现 writer drain → 上游拆卸。详见
[dashboard L3](../L3-details/dashboard.md) / [L4](./dashboard.md)。

### 全局
- `--catalog <path>`：工具目录 JSON（默认指向测试 fixture，见 L3 说明）。**仅** `search`/`get-details` 使用。
- `--config <path>`：可选 TOML 配置；省略则用全默认。

> 行为与测试细节见 L3：[mcpgw-cli](../L3-details/mcpgw-cli.md)
