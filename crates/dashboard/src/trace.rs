// `DiscoveryHit` is used only by the test module (via `super::*`); keep it in the public-facing
// import block for this discovery module and silence the non-test unused-import lint.
#[allow(unused_imports)]
use observe::{DiscoveryHit, DiscoveryRecord, DiscoverySink};
use std::collections::VecDeque;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;

const WRITER_CHANNEL_CAP: usize = 1024;

/// Join handle for the optional discovery-JSONL writer thread. Dropping all `DiscoveryRingSink`
/// clones closes the channel; `join()` then drains and flushes remaining lines.
pub struct DiscoveryWriter {
    handle: std::thread::JoinHandle<()>,
}

impl DiscoveryWriter {
    pub fn join(self) {
        let _ = self.handle.join();
    }
}

/// In-memory ring buffer of recent discovery traces (newest-first on read), with an optional
/// background writer appending each record as a JSON line to a discovery JSONL file.
pub struct DiscoveryRingSink {
    cap: usize,
    ring: Mutex<VecDeque<DiscoveryRecord>>,
    tx: Option<SyncSender<String>>,
    dropped: AtomicU64,
}

impl DiscoveryRingSink {
    /// Build a ring sink (capacity `cap`). When `path` is `Some`, also append records to that
    /// discovery JSONL via a background writer thread (returned for graceful drain on shutdown).
    pub fn spawn(
        cap: usize,
        path: Option<&Path>,
    ) -> std::io::Result<(Self, Option<DiscoveryWriter>)> {
        let cap = cap.max(1);
        let (tx, writer) = match path {
            None => (None, None),
            Some(p) => {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)?;
                let (tx, rx) = sync_channel::<String>(WRITER_CHANNEL_CAP);
                let handle = std::thread::Builder::new()
                    .name("discovery-writer".into())
                    .spawn(move || run_writer(rx, file))?;
                (Some(tx), Some(DiscoveryWriter { handle }))
            }
        };
        Ok((
            Self {
                cap,
                ring: Mutex::new(VecDeque::with_capacity(cap)),
                tx,
                dropped: AtomicU64::new(0),
            },
            writer,
        ))
    }

    /// Most recent records, newest first, capped at `limit`.
    pub fn recent(&self, limit: usize) -> Vec<DiscoveryRecord> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().rev().take(limit).cloned().collect()
    }

    /// Count of records dropped because the writer channel was full (test/diagnostics).
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl DiscoverySink for DiscoveryRingSink {
    fn record(&self, rec: &DiscoveryRecord) {
        {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            if ring.len() == self.cap {
                ring.pop_front();
            }
            ring.push_back(rec.clone());
        }
        if let Some(tx) = &self.tx {
            if let Ok(line) = serde_json::to_string(rec) {
                if let Err(TrySendError::Full(_)) = tx.try_send(line) {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Append each received line + flush; on clean disconnect, flush and fsync once before exit.
fn run_writer(rx: Receiver<String>, file: std::fs::File) {
    let mut w = BufWriter::new(file);
    while let Ok(line) = rx.recv() {
        if writeln!(w, "{line}").is_err() {
            continue;
        }
        while let Ok(next) = rx.try_recv() {
            let _ = writeln!(w, "{next}");
        }
        let _ = w.flush();
    }
    let _ = w.flush();
    if let Ok(file) = w.into_inner() {
        let _ = file.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(q: &str) -> DiscoveryRecord {
        DiscoveryRecord {
            ts_unix_ms: 0,
            query: q.into(),
            top_k: 1,
            results: vec![DiscoveryHit {
                name: "s__t".into(),
                score: 1.0,
            }],
            latency_ms: 0,
        }
    }

    #[test]
    fn ring_caps_and_returns_newest_first() {
        let (sink, _w) = DiscoveryRingSink::spawn(2, None).unwrap();
        sink.record(&rec("a"));
        sink.record(&rec("b"));
        sink.record(&rec("c")); // evicts "a"
        let recent = sink.recent(10);
        let queries: Vec<_> = recent.iter().map(|r| r.query.as_str()).collect();
        assert_eq!(queries, ["c", "b"], "newest first, capacity 2");
    }

    #[test]
    fn recent_respects_limit() {
        let (sink, _w) = DiscoveryRingSink::spawn(10, None).unwrap();
        for q in ["a", "b", "c"] {
            sink.record(&rec(q));
        }
        assert_eq!(sink.recent(2).len(), 2);
    }

    #[test]
    fn file_writer_persists_lines_then_drains_on_join() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mcpgw-disc-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (sink, writer) = DiscoveryRingSink::spawn(10, Some(&path)).unwrap();
        sink.record(&rec("x"));
        sink.record(&rec("y"));
        drop(sink); // release the sender so the writer thread can finish
        writer.expect("writer present when path given").join();
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["query"], "x");
        let _ = std::fs::remove_file(&path);
    }
}
