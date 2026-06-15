# M6.T3 审计落库（append-only JSONL）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 M6.T1 已产出的、仅元数据的 `observe::CallRecord` 持久化为可选的 append-only JSONL 审计文件，由后台 OS 线程异步落盘，`[audit]` 开关控制。

**Architecture:** 新增 `observe::audit` 模块——`JsonlSink`（实现 `CallSink`，`record()` 同步非阻塞 `try_send` 一行 JSON 到 bounded `sync_channel`）+ 一条专用 OS 线程（`BufWriter<File>` append 落盘、批量 flush、关闭时 fsync）。`config` 加 `[audit]` 段；`mcpgw` 据其在 `run_serve` 装配 `JsonlSink`（打开失败即 fail-fast），`select!` 收尾后 drop sinks 触发优雅 drain，并以有界超时 join writer。`observe` 保持 std-only，无新依赖。

**Tech Stack:** Rust，`std::sync::mpsc::sync_channel` + `std::thread` + `std::io::BufWriter`，`serde_json`（已有），`tracing`（已有）。

> 设计依据：`docs/superpowers/specs/2026-06-15-mcpgw-m6t3-audit-jsonl-design.md`。前置 M6.T1 已合并。

---

## File Structure

| 文件 | 动作 | 职责 |
|------|------|------|
| `crates/observe/src/audit.rs` | **新建** | `JsonlSink` / `AuditWriter` / `spawn_writer` / `AUDIT_CHANNEL_CAPACITY` + 后台 writer loop + 单元测试 |
| `crates/observe/src/lib.rs` | 改 | `mod audit;` + `pub use audit::{...}` |
| `crates/config/src/lib.rs` | 改 | `AuditConfig{enabled,path}` + `Config.audit` 字段 + 测试 |
| `crates/mcpgw/src/main.rs` | 改 | `run_serve`：按 `[audit]` 装配 `JsonlSink`（fail-fast）、收尾有界优雅 drain |
| `crates/mcpgw/tests/cli.rs` 或新 `tests/audit.rs` | 改/新建 | 集成：启用 audit → 起 serve → 调用 → 关闭 → 断言文件有合法 JSONL 行 |
| `docs/L4-api/observe-audit.md` | **新建** | L4 逐文件 API |
| `docs/L2-components/observe.md`、`docs/L2-components/config.md`、`docs/L3-details/config.md`、`docs/L4-api/config-lib.md`、`docs/L2-components/mcpgw-cli.md`、`docs/L3-details/mcpgw-cli.md`、`docs/L4-api/mcpgw-main.md`、`docs/L1-overview.md`、`docs/README.md`、`docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md` | 改 | 分层文档同步 |

**前置：建分支**

```bash
cd /home/verden/course/mcpgw
git checkout master
git checkout -b feat/m6t3-audit-jsonl
```

---

### Task 1: `observe::audit` 模块（`JsonlSink` + 后台 writer + `spawn_writer`）

**Files:**
- Create: `crates/observe/src/audit.rs`
- Modify: `crates/observe/src/lib.rs`（加 `mod audit;` 与 re-export）
- Test: `crates/observe/src/audit.rs` 内 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败测试（先建 audit.rs，含实现骨架 + 测试）**

> 说明：Rust 单元测试与被测代码同文件，无法"先只写测试再让它编译失败"。本任务用 TDD 的节奏是：先写最小实现 + 测试，跑测试看红/绿。先创建 `crates/observe/src/audit.rs`，内容如下（实现 + 测试一并给出，便于一次成型）：

```rust
//! 可选的 JSONL 审计 sink。
//!
//! `JsonlSink` 是一个 `CallSink`：`record()` 把一条仅元数据的 `CallRecord` 序列化成一行 JSON 并
//! `try_send` 进 bounded channel；一条专用 OS 线程持 receiver 做阻塞文件 I/O（append + 缓冲写），
//! 故调用热路径绝不阻塞。保持仅元数据、std-only（不引入 tokio），`observe` 不新增依赖。

use crate::{CallRecord, CallSink};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// `JsonlSink::record` 与 writer 线程间的 bounded channel 容量。满则丢弃（绝不阻塞调用）。
pub const AUDIT_CHANNEL_CAPACITY: usize = 1024;

/// 经后台 writer 线程把每条记录 append 成一行 JSON 的 `CallSink`。
#[derive(Clone)]
pub struct JsonlSink {
    tx: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl JsonlSink {
    /// 至今因 channel 满/断连而丢弃的记录数。
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl CallSink for JsonlSink {
    fn record(&self, rec: &CallRecord) {
        let line = match serde_json::to_string(rec) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "audit: serialize CallRecord failed; dropping");
                return;
            }
        };
        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                // 限频：首次（n==1）及之后每个 2 的幂次打印累计丢弃数。
                if n.is_power_of_two() {
                    tracing::warn!(dropped = n, "audit: channel full/closed; dropping record(s)");
                }
            }
        }
    }
}

/// 后台 writer 线程句柄。`join` 阻塞直到 writer drain 完 channel、flush、fsync 并退出——这在每个
/// `JsonlSink` 克隆都被 drop 后发生。
pub struct AuditWriter {
    handle: JoinHandle<()>,
}

impl AuditWriter {
    /// 阻塞直到 writer 线程结束。
    pub fn join(self) {
        let _ = self.handle.join();
    }
}

/// 内部：造一个 `JsonlSink` 及配套 receiver。暴露给测试，以便其持有未消费的 receiver、确定性地
/// 触发 channel-full 丢弃路径。
pub(crate) fn channel(capacity: usize) -> (JsonlSink, Receiver<String>) {
    let (tx, rx) = sync_channel(capacity);
    (
        JsonlSink {
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
        },
        rx,
    )
}

/// 以 append 方式打开 `path`（不存在则创建），spawn writer 线程，返回 sink 与其句柄。打不开文件即
/// `Err`（调用方据此 fail-fast）。
pub fn spawn_writer(path: &Path, capacity: usize) -> std::io::Result<(JsonlSink, AuditWriter)> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let (sink, rx) = channel(capacity);
    let handle = std::thread::Builder::new()
        .name("audit-writer".into())
        .spawn(move || run_writer(rx, file))
        .expect("spawn audit-writer thread");
    Ok((sink, AuditWriter { handle }))
}

/// writer 主循环：逐行 append、批量 drain 已排队项、按批 flush；当 channel 断连（所有 sender 被 drop）
/// 且队列已 FIFO drain 完，做最后一次 flush + fsync 后退出。写失败只限频 warn 且**不**退出——瞬时
/// 故障（如满盘后清理）可自愈。
fn run_writer(rx: Receiver<String>, file: File) {
    let mut w = BufWriter::new(file);
    let mut write_errors: u64 = 0;
    while let Ok(line) = rx.recv() {
        write_line(&mut w, &line, &mut write_errors);
        while let Ok(more) = rx.try_recv() {
            write_line(&mut w, &more, &mut write_errors);
        }
        if let Err(e) = w.flush() {
            rate_limited_write_error(&mut write_errors, &e);
        }
    }
    let _ = w.flush();
    if let Ok(file) = w.into_inner() {
        let _ = file.sync_all();
    }
}

fn write_line(w: &mut BufWriter<File>, line: &str, errors: &mut u64) {
    if let Err(e) = w.write_all(line.as_bytes()).and_then(|_| w.write_all(b"\n")) {
        rate_limited_write_error(errors, &e);
    }
}

fn rate_limited_write_error(errors: &mut u64, e: &std::io::Error) {
    *errors += 1;
    if errors.is_power_of_two() {
        tracing::warn!(errors = *errors, error = %e, "audit: write failed; keeping writer alive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallOutcome, MetaTool};

    fn sample() -> CallRecord {
        CallRecord {
            ts_unix_ms: 1,
            meta_tool: MetaTool::CallTool,
            target_tool: Some("github__create_issue".into()),
            upstream: Some("github".into()),
            latency_ms: 7,
            outcome: CallOutcome::Ok,
            error_kind: None,
            arg_bytes: 42,
            result_bytes: 99,
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("mcpgw-audit-test-{tag}-{nanos}.jsonl"))
    }

    #[test]
    fn writes_n_records_as_valid_jsonl_and_drains_on_drop() {
        let path = temp_path("write");
        let (sink, writer) = spawn_writer(&path, 64).unwrap();
        for _ in 0..5 {
            sink.record(&sample());
        }
        drop(sink); // 最后一个 sender 消失 -> writer drain、flush、fsync、退出
        writer.join();

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 5, "每条记录一行 JSON");
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["meta_tool"], "call_tool");
            assert_eq!(v["arg_bytes"], 42);
            assert!(v.get("arguments").is_none(), "审计行绝不含 payload");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn channel_full_increments_dropped_without_blocking() {
        // 持有未消费的 receiver，使 channel 确定性地填满。
        let (sink, _rx) = channel(1);
        sink.record(&sample()); // 填满唯一槽位
        sink.record(&sample()); // Full -> 丢弃
        sink.record(&sample()); // Full -> 丢弃
        assert_eq!(sink.dropped_count(), 2);
    }

    #[test]
    fn spawn_writer_open_failure_returns_err() {
        let bad = std::path::Path::new("/nonexistent-dir-mcpgw-xyz/audit.jsonl");
        assert!(spawn_writer(bad, 8).is_err());
    }
}
```

- [ ] **Step 2: 接线 lib.rs**

在 `crates/observe/src/lib.rs` 顶部（`use serde::Serialize;` 之后）加模块声明与 re-export：

```rust
mod audit;
pub use audit::{spawn_writer, AuditWriter, JsonlSink, AUDIT_CHANNEL_CAPACITY};
```

- [ ] **Step 3: 跑测试看绿**

Run: `cargo test -p observe`
Expected: 新增 3 个测试 PASS（`writes_n_records_as_valid_jsonl_and_drains_on_drop`、`channel_full_increments_dropped_without_blocking`、`spawn_writer_open_failure_returns_err`），原有 observe 测试仍 PASS。

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt -p observe && cargo fmt --check -p observe && cargo clippy -p observe --all-targets --all-features -- -D warnings`
Expected: 无改动残留、无 clippy 告警。

- [ ] **Step 5: Commit**

```bash
git add crates/observe/src/audit.rs crates/observe/src/lib.rs
git commit -m "feat(observe): JsonlSink + background writer thread for audit (M6.T3 T1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 2: `[audit]` 配置段

**Files:**
- Modify: `crates/config/src/lib.rs`（加 `AuditConfig` + `Config.audit` 字段 + 测试）

- [ ] **Step 1: 写失败测试**

在 `crates/config/src/lib.rs` 的 `#[cfg(test)] mod tests` 内追加 4 个测试：

```rust
#[test]
fn audit_defaults_disabled() {
    let cfg = Config::from_toml_str("").unwrap();
    assert!(!cfg.audit.enabled);
    assert_eq!(cfg.audit.path, "mcpgw-audit.jsonl");
}

#[test]
fn parses_audit_section() {
    let cfg = Config::from_toml_str(
        "[audit]\nenabled = true\npath = \"/var/log/mcpgw/audit.jsonl\"\n",
    )
    .unwrap();
    assert!(cfg.audit.enabled);
    assert_eq!(cfg.audit.path, "/var/log/mcpgw/audit.jsonl");
}

#[test]
fn audit_rejects_unknown_field() {
    let err = Config::from_toml_str("[audit]\nbogus = 1\n").unwrap_err();
    assert!(matches!(err, ConfigError::Parse(_)));
}

#[test]
fn audit_partial_fills_defaults() {
    // 只给 enabled -> path 保持默认。
    let cfg = Config::from_toml_str("[audit]\nenabled = true\n").unwrap();
    assert!(cfg.audit.enabled);
    assert_eq!(cfg.audit.path, "mcpgw-audit.jsonl");
}
```

- [ ] **Step 2: 跑测试看失败**

Run: `cargo test -p config audit`
Expected: 编译失败 / 测试失败——`Config` 尚无 `audit` 字段、`AuditConfig` 未定义。

- [ ] **Step 3: 加 `audit` 字段到 `Config`**

把 `crates/config/src/lib.rs` 的 `Config` 结构体改为（在 `server` 字段后加 `audit`）：

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default, rename = "upstream")]
    pub upstreams: Vec<UpstreamConfig>,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub audit: AuditConfig,
}
```

- [ ] **Step 4: 定义 `AuditConfig`**

在 `ServerConfig`/`HttpConfig` 等结构体附近（紧随 `HttpConfig` 之后即可）插入：

```rust
/// `[audit]` 段：可选的、append-only JSONL 元工具调用审计日志（M6.T3）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditConfig {
    /// 每次元工具调用写一行审计。默认 false（需显式开启）。
    pub enabled: bool,
    /// 审计文件路径（append-only JSONL）。每个网关进程需独占自己的路径。
    pub path: String,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "mcpgw-audit.jsonl".into(),
        }
    }
}
```

> 无需改 `validate()`：path 内容不校验，打开失败在 `mcpgw` 启动期 fail-fast 暴露（Task 3）。

- [ ] **Step 5: 跑测试看绿**

Run: `cargo test -p config`
Expected: 4 个新测试 PASS，原有 config 测试全 PASS。

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt -p config && cargo fmt --check -p config && cargo clippy -p config --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 7: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): [audit] section (enabled/path), default disabled (M6.T3 T2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 3: `mcpgw` 装配 + 优雅 drain + 端到端测试

**Files:**
- Modify: `crates/mcpgw/src/main.rs`（`run_serve`：装配 `JsonlSink` fail-fast + 收尾有界 drain）
- Create: `crates/mcpgw/tests/audit.rs`（端到端：起 serve + audit → 调用 → 关闭 → 断言文件）

- [ ] **Step 1: 写失败的端到端测试**

新建 `crates/mcpgw/tests/audit.rs`：

```rust
//! End-to-end: `mcpgw serve` with `[audit]` enabled must append a metadata-only JSONL line per
//! meta-tool call, and flush it during graceful shutdown.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::json;
use tokio::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpgw")
}

fn unique_temp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("mcpgw-{tag}-{nanos}"))
}

#[tokio::test]
async fn serve_with_audit_enabled_writes_jsonl_for_a_meta_tool_call() {
    let audit_path = unique_temp("audit-it.jsonl");
    let cfg_path = unique_temp("audit-it-config.toml");
    {
        let mut f = std::fs::File::create(&cfg_path).unwrap();
        // stdio only (no http) so shutdown is driven purely by stdin EOF on client cancel.
        writeln!(
            f,
            "[server]\nstdio = true\n\n[audit]\nenabled = true\npath = {:?}\n",
            audit_path.to_str().unwrap()
        )
        .unwrap();
    }

    // Spawn `mcpgw serve --config <cfg>` and connect an MCP client over its stdio.
    let transport = TokioChildProcess::new(Command::new(bin()).configure(|c| {
        c.arg("serve")
            .arg("--config")
            .arg(&cfg_path)
            .stderr(Stdio::null()); // avoid stderr pipe backpressure during the test
    }))
    .unwrap();
    let client = ().serve(transport).await.unwrap();

    // search_tools records even with no upstreams (empty snapshot -> still a meta-tool call).
    client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(json!({"query": "anything"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();

    // Disconnect: closing the child's stdin drives run_serve's graceful shutdown + audit drain.
    client.cancel().await.unwrap();

    // The writer flushes+fsyncs during graceful drain before the process exits; poll briefly.
    let mut body = String::new();
    for _ in 0..30 {
        if let Ok(s) = std::fs::read_to_string(&audit_path) {
            if !s.trim().is_empty() {
                body = s;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "expected at least one audit line; file was: {body:?}"
    );
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["meta_tool"], "search_tools");
    assert!(
        v.get("arguments").is_none(),
        "audit line must not contain payloads"
    );

    let _ = std::fs::remove_file(&audit_path);
    let _ = std::fs::remove_file(&cfg_path);
}
```

- [ ] **Step 2: 跑测试看失败**

Run: `cargo test -p mcpgw --test audit`
Expected: 失败——`run_serve` 尚未据 `[audit]` 装配 `JsonlSink`，审计文件不会被创建/写入（poll 超时后 `lines.is_empty()` 断言失败）。

- [ ] **Step 3: 在 `run_serve` 装配 `JsonlSink`（fail-fast）**

把 `crates/mcpgw/src/main.rs` 中现有的 sinks 构造块：

```rust
    // Observation sinks shared by both stdio and http transports.
    let sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> =
        vec![std::sync::Arc::new(observe::TracingSink) as std::sync::Arc<dyn observe::CallSink>]
            .into();
```

替换为：

```rust
    // Observation sinks shared by both stdio and http transports. Default = TracingSink;
    // when [audit].enabled, additionally append a JsonlSink backed by a background writer.
    let mut sink_vec: Vec<std::sync::Arc<dyn observe::CallSink>> =
        vec![std::sync::Arc::new(observe::TracingSink) as std::sync::Arc<dyn observe::CallSink>];
    let audit_writer = if cfg.audit.enabled {
        let (sink, writer) = observe::spawn_writer(
            std::path::Path::new(&cfg.audit.path),
            observe::AUDIT_CHANNEL_CAPACITY,
        )
        .map_err(|e| format!("open audit file {:?}: {e}", cfg.audit.path))?;
        tracing::info!(path = %cfg.audit.path, "audit log enabled");
        sink_vec.push(std::sync::Arc::new(sink));
        Some(writer)
    } else {
        None
    };
    let sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> = sink_vec.into();
```

- [ ] **Step 4: 收尾处加有界优雅 drain**

在 `crates/mcpgw/src/main.rs` 的 `let outcome: Result<(), String> = tokio::select! { ... };` **之后**、上游 shutdown 循环之前，插入：

```rust
    // Drain the audit writer (if any): all GatewayServer sink clones were dropped when the
    // select! branches were dropped, so dropping our own `sinks` releases the last JsonlSink
    // clone and disconnects the channel; the writer then FIFO-drains, flushes, fsyncs, and exits.
    // The bounded timeout covers a lingering in-flight HTTP connection that still holds a clone.
    drop(sinks);
    if let Some(writer) = audit_writer {
        if tokio::time::timeout(
            AUDIT_DRAIN_TIMEOUT,
            tokio::task::spawn_blocking(move || writer.join()),
        )
        .await
        .is_err()
        {
            tracing::warn!("audit writer drain timed out; some records may be unflushed");
        }
    }
```

- [ ] **Step 5: 定义 `AUDIT_DRAIN_TIMEOUT` 常量**

在 `crates/mcpgw/src/main.rs` 顶部（其他 `use`/常量附近）加：

```rust
/// Upper bound on how long shutdown waits for the audit writer to drain + fsync.
const AUDIT_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
```

- [ ] **Step 6: 跑端到端测试看绿**

Run: `cargo test -p mcpgw --test audit`
Expected: PASS——审计文件含 ≥1 行，首行 `meta_tool == "search_tools"`，无 `arguments` 键。

- [ ] **Step 7: 跑全 crate 测试 + fmt + clippy**

Run: `cargo test -p mcpgw && cargo fmt -p mcpgw && cargo fmt --check -p mcpgw && cargo clippy -p mcpgw --all-targets --all-features -- -D warnings`
Expected: 全 PASS、干净、无告警。

- [ ] **Step 8: Commit**

```bash
git add crates/mcpgw/src/main.rs crates/mcpgw/tests/audit.rs
git commit -m "feat(mcpgw): assemble JsonlSink from [audit] + bounded graceful drain (M6.T3 T3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: 分层文档（L1–L4 + README + roadmap）

docs 必须忠实描述已落地代码——动手前先读对应源码与现有 doc 风格（如 `docs/L4-api/observe-lib.md`、`docs/L2-components/observe.md`、`docs/L4-api/config-lib.md`、`docs/L3-details/config.md`）。

**Files:**
- Create: `docs/L4-api/observe-audit.md`
- Modify: `docs/L2-components/observe.md`、`docs/L2-components/config.md`、`docs/L3-details/config.md`、`docs/L4-api/config-lib.md`、`docs/L2-components/mcpgw-cli.md`、`docs/L3-details/mcpgw-cli.md`、`docs/L4-api/mcpgw-main.md`、`docs/L1-overview.md`、`docs/README.md`、`docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`

- [ ] **Step 1: 新建 L4 `docs/L4-api/observe-audit.md`**

覆盖 `crates/observe/src/audit.rs`：
- `AUDIT_CHANNEL_CAPACITY`（bounded channel 容量常量 = 1024，满则丢弃）。
- `JsonlSink`（`impl CallSink`；`record()` = `serde_json::to_string` + `try_send`；`Full`/`Disconnected` → `dropped` 计数 + 限频 warn（首次及每个 2 的幂次）；`dropped_count()`；`Clone` 共享同一 sender 与计数）。
- `AuditWriter`（持后台线程 `JoinHandle`；`join(self)` 阻塞至 writer drain+flush+fsync+退出；**不持 sender**，故 drain 触发是"所有 `JsonlSink` 克隆 drop"）。
- `spawn_writer(path, capacity) -> io::Result<(JsonlSink, AuditWriter)>`（`create+append` 打开；打不开即 `Err`）。
- writer 语义：批量 drain + 按批 `flush`（→OS）、关闭时 `flush`+`sync_all`（fsync）；写失败限频 warn 且**不退出**（自愈）。
- 仅元数据不变量：只序列化 `CallRecord`，从类型上无 payload。
- 风格仿 `observe-lib.md`：`源文件:` 引子 + 逐项 `##` + ```rust 签名 + 表格 + `>` 导航脚注。脚注链接回 L2 observe。

- [ ] **Step 2: 更新 L2 `docs/L2-components/observe.md`**

- 职责补："可选的 JSONL 审计落盘（`JsonlSink` + 专用 OS 线程 writer，std-only）"。
- 公开接口表加 `JsonlSink` / `spawn_writer` / `AuditWriter` / `AUDIT_CHANNEL_CAPACITY`。
- 依赖小节明确：审计 writer 用 **`std::thread` + `std::sync::mpsc`**，**不**引入 `tokio`（保持 std-only 的现有表述）。
- "被谁使用"补：`mcpgw` 据 `[audit]` 装配 `JsonlSink`。
- "不负责"更新：存储已由 T3 落地（JSONL），仍不含指标（M6.T2）。
- 向下导航加 L4 [observe-audit](../L4-api/observe-audit.md)。

- [ ] **Step 3: 更新 config 文档（L2/L3/L4）**

- `docs/L4-api/config-lib.md`：加 `AuditConfig { enabled: bool=false, path: String="mcpgw-audit.jsonl" }` 与 `Config.audit` 字段；`#[serde(default, deny_unknown_fields)]`、省略段=关闭。
- `docs/L2-components/config.md`：配置项清单加 `[audit]` 段（用途：可选 JSONL 审计落盘开关 + 路径）。
- `docs/L3-details/config.md`：加"`[audit]` 段"小节——字段语义、默认、**运维轮转说明**：无内置轮转/SIGHUP 重开；外部 `logrotate`（① `copytruncate` 无停机但 copy↔truncate 间的行可能丢失；② 停机轮转零丢失）；单进程单 writer，多进程同文件属误配。

- [ ] **Step 4: 更新 mcpgw-cli L2/L3 + mcpgw-main L4**

- `docs/L2-components/mcpgw-cli.md`：`serve` 描述补"`[audit].enabled` 时装配 `JsonlSink`（打开文件 fail-fast）、关闭时有界优雅 drain 审计 writer"；内部依赖小节已含 `observe`（无需改）。
- `docs/L3-details/mcpgw-cli.md`：`run_serve` 流程补"按 `[audit]` 追加 `JsonlSink` 并持 `AuditWriter`；`select!` 后 `drop(sinks)` 触发断连 → `tokio::time::timeout(AUDIT_DRAIN_TIMEOUT, spawn_blocking(writer.join()))`；drop 顺序要求与超时兜底悬挂的 http 连接 sink 克隆"。
- `docs/L4-api/mcpgw-main.md`：装配/收尾细节加 audit 分支与 `AUDIT_DRAIN_TIMEOUT`。

- [ ] **Step 5: 更新 L1 `docs/L1-overview.md`**

- `observe` crate 描述补"可选 JSONL 审计落盘（M6.T3）"；若有架构图/能力清单补上。
- 加"M6.T3 已完成"里程碑小结（仿 M6.T1）。**默认仍 bm25、审计默认关闭**。
- 测试计数块按 `cargo test --all-features` 实测更新（observe 新增 3、config 新增 4、mcpgw 新增 1 个集成测试——以实跑为准重算总数与分项）。

- [ ] **Step 6: 更新 README + roadmap**

- `docs/README.md`：L4 清单加 `observe-audit.md`；里程碑覆盖说明加 **M6.T3（审计落库 JSONL）**。
- `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`：`M6.T3` 标 `✅ 已完成（JsonlSink + 后台 writer + [audit] 配置；append-only、仅元数据）`；可注"M6.T2 指标、T4 code-mode 延后"。

- [ ] **Step 7: 校对 + 提交**

- 逐项核对新/改 doc 与真实代码一致（audit 类型/签名、config 字段/默认、mcpgw 装配与 drain、`AUDIT_DRAIN_TIMEOUT`）。
- 确认新增内链指向真实文件：`ls docs/L4-api/observe-audit.md`。
- 产品文档（L1–L4 + README）无残留"observe 仅 tracing、无落盘"的旧表述。

```bash
git add docs/
git commit -m "docs: L1-L4 + README + roadmap for M6.T3 audit JSONL (M6.T3 T4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: 全量验证 + 合回 master

- [ ] **Step 1: 全量验证**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净；clippy 无告警；全测试 PASS（含 observe audit 3 个单测、config 4 个新测、mcpgw `tests/audit.rs` 端到端；`#[ignore]` 真实冒烟仍跳过）。记录总数用于 L1 测试计数块复核。

- [ ] **Step 2: 最终整体 code review**

派发最终 whole-feature review（模型用当前主会话模型），关注：
- 仅元数据不变量端到端成立（`JsonlSink` 只序列化 `CallRecord`）；
- sink `record` 同步/非阻塞/不 panic；channel 满丢弃 + 限频 warn；
- writer 写失败保活、关闭 flush+fsync；drain 触发依赖全 sender drop 且超时兜底；
- `mcpgw` 装配 fail-fast、收尾 drop 顺序正确、全调用点无回归；
- 文档与代码同步、运维轮转说明准确。
处理 blocking 项（如有），小提交折叠 review nits。

- [ ] **Step 3: 收尾合并**

用 superpowers:finishing-a-development-branch 把 `feat/m6t3-audit-jsonl` 合回 master（`--no-ff`，本地），合并后在 master 复跑 `cargo test --all-features` 确认绿，再删分支。

## 实现期需现场确认/可能回退的点

- **端到端测试关闭时序**：`client.cancel()` 后子进程需 drain+fsync 再退出；测试以最多 3s 轮询读文件兜底。若 CI 上 stdio EOF→关闭偏慢，放宽轮询次数；仍失败则退化为 spec §8 的轻量装配测试（`run_serve` 在 `enabled=true` 下成功建出含 `JsonlSink` 的 sinks 且能优雅收尾）。
- **drop 顺序**：依赖"select! 结束后两个 `GatewayServer` 的 sink 克隆已 drop、再 `drop(sinks)`"。如发现 axum 在 select! 后仍保留连接级 sink 克隆，靠 `AUDIT_DRAIN_TIMEOUT` 兜底（已设计）。
- **限频 warn 策略**（首次 + 2 的幂次）以代码实测为准。
- **临时文件测试**：用 `std::env::temp_dir()` + 唯一名 + 结束清理；若仓库已引入 `tempfile` 可复用以更稳。
- **`TokioChildProcess` stderr**：测试中 `stderr(Stdio::null())` 避免 stderr 管道背压；确认 rmcp 1.7 的 `ConfigureCommandExt` 提供 `.configure`。

