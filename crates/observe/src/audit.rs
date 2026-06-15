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
                    tracing::warn!(
                        dropped = n,
                        "audit: channel full/closed; dropping record(s)"
                    );
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
    if let Err(e) = w
        .write_all(line.as_bytes())
        .and_then(|_| w.write_all(b"\n"))
    {
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
