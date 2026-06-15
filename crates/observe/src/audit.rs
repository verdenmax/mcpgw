//! Optional JSONL audit sink.
//!
//! `JsonlSink` is a `CallSink`: `record()` serializes one metadata-only `CallRecord` to a single
//! JSON line and `try_send`s it into a bounded channel; a dedicated OS thread owns the receiver and
//! does the blocking file I/O (append + buffered writes), so the call hot-path never blocks. Stays
//! metadata-only and std-only (no tokio) — `observe` gains no new dependency.

use crate::{CallRecord, CallSink};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Bounded channel capacity between `JsonlSink::record` and the writer thread. When full, records
/// are dropped (the call path is never blocked).
pub const AUDIT_CHANNEL_CAPACITY: usize = 1024;

/// A `CallSink` that appends each record as one JSON line via a background writer thread.
#[derive(Clone)]
pub struct JsonlSink {
    tx: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl JsonlSink {
    /// Number of records dropped so far because the channel was full or disconnected.
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

/// Handle to the background writer thread. `join` blocks until the writer drains the channel,
/// flushes, fsyncs, and exits — which happens once every `JsonlSink` clone has been dropped.
pub struct AuditWriter {
    handle: JoinHandle<()>,
}

impl AuditWriter {
    /// Block until the writer thread finishes.
    pub fn join(self) {
        let _ = self.handle.join();
    }
}

/// Internal: build a `JsonlSink` and its matching receiver. Exposed to tests so they can hold the
/// receiver unread and deterministically exercise the channel-full drop path.
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

/// Open `path` for append (creating it if absent), spawn the writer thread, and return the sink
/// plus its handle. Returns `Err` if the file cannot be opened, or if the writer thread cannot be
/// spawned (so the caller can fail-fast).
pub fn spawn_writer(path: &Path, capacity: usize) -> std::io::Result<(JsonlSink, AuditWriter)> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let (sink, rx) = channel(capacity);
    let handle = std::thread::Builder::new()
        .name("audit-writer".into())
        .spawn(move || run_writer(rx, file))?;
    Ok((sink, AuditWriter { handle }))
}

/// Writer loop: append each line, batch-drain whatever is queued, flush per batch; when the channel
/// disconnects (all senders dropped) and the queue is FIFO-drained, do a final flush + fsync and
/// exit. Write errors only rate-limit-warn and do NOT stop the writer — transient faults (e.g. a
/// full disk that later clears) self-heal.
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
    if let Err(e) = w.flush() {
        rate_limited_write_error(&mut write_errors, &e);
    }
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
