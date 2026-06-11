# L4 — `crates/upstream/src/connect.rs` API

源文件：`crates/upstream/src/connect.rs`。按配置 **eager-connect** 所有上游（真实 stdio 子进程），采用
**降级启动（degraded start）**：单个上游连接失败不阻断其余。

## `struct ConnectSummary`
```rust
pub struct ConnectSummary {
    pub connected: Vec<String>,             // 成功建连并插入注册表的上游名
    pub skipped: Vec<(String, String)>,     // 跳过的上游 (name, 原因文本)
}
```
`connect_all` 的结果：哪些上游连上、哪些被跳过（含原因）。

## `fn build_command`（crate-internal）
```rust
pub(crate) fn build_command(cfg: &UpstreamConfig) -> tokio::process::Command
```
为 stdio 上游构造子进程命令，并施加 **env allow-list**：先 `c.env_clear()` 清空子进程环境，再仅把
`env_passthrough` 列出、且在 mcpgw 自身环境里存在的变量逐个传入。`pub(crate)` 暴露仅供单测验证 allow-list
行为（`build_command_applies_env_allowlist`）。无错误（构造期）。

## `async fn connect_stdio_upstream`
```rust
pub async fn connect_stdio_upstream(
    cfg: &UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError>
```
spawn 一个 stdio 子进程并连接：

1. `build_command(cfg)` → `TokioChildProcess::new(cmd)`（失败 → `UpstreamError::Connect`）。
2. `UpstreamHandle::connect_with_trigger(&cfg.name, transport, trigger)`，并用
   `tokio::time::timeout(cfg.call_timeout_ms, connect)` 给 **connect/initialize 握手**加界——子进程起得来但
   不应答时不会挂死降级启动；超时 → `UpstreamError::Timeout`。
3. 成功后 `handle.with_call_timeout(cfg.call_timeout_ms)` 设置每调用超时。

`trigger` 透传给 handler：连上后该上游的 `tools/list_changed` 会推动网关重建。

## `async fn connect_all`
```rust
pub async fn connect_all(
    registry: &UpstreamRegistry,
    upstreams: &[UpstreamConfig],
    trigger: RebuildTrigger,
) -> ConnectSummary
```
顺序遍历每个 `UpstreamConfig`，调 `connect_stdio_upstream(cfg, Some(trigger.clone()))`：

- **成功** → `registry.insert(Arc::new(handle))`，记入 `summary.connected`。
- **失败** → `tracing::warn!("connect failed; skipping")`，记入 `summary.skipped`（不返回 `Err`）。

即**降级启动**：返回 `ConnectSummary` 而非 `Result`——某上游连不上只被记录、不阻断网关启动；`serve` 据此打日志后
继续做初始 rebuild。

> 详见 L3：[upstream](../L3-details/upstream.md)；连接句柄/触发器见 L4：[connection](./upstream-connection.md)
