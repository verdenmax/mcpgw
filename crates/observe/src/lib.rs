//! `observe`: structured, metadata-only observation of gateway meta-tool calls.
//!
//! Defines `CallRecord` (NO argument/result payloads — only sizes), the `CallSink` trait, and a
//! `TracingSink`. This is the storage-free, HTTP-free seam that T1 (tracing) and T3 (audit JSONL)
//! share: one record is built at the call boundary and fanned out to every configured sink.

use serde::Serialize;

mod audit;
pub use audit::{spawn_writer, AuditWriter, JsonlSink, AUDIT_CHANNEL_CAPACITY};

/// Which meta-tool was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaTool {
    SearchTools,
    GetToolDetails,
    CallTool,
}

impl MetaTool {
    /// The canonical snake_case token (identical to the serde representation), so the tracing and
    /// JSONL sinks describe the same record with the same spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            MetaTool::SearchTools => "search_tools",
            MetaTool::GetToolDetails => "get_tool_details",
            MetaTool::CallTool => "call_tool",
        }
    }
}

/// The outcome of a meta-tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Ok,
    Error,
    Timeout,
}

impl CallOutcome {
    /// The canonical snake_case token (identical to the serde representation).
    pub fn as_str(&self) -> &'static str {
        match self {
            CallOutcome::Ok => "ok",
            CallOutcome::Error => "error",
            CallOutcome::Timeout => "timeout",
        }
    }
}

/// Metadata-only record of one meta-tool call. By construction it carries NO argument or result
/// content — only sizes (`arg_bytes`/`result_bytes`) — so it can never leak secrets/PII into logs
/// or the audit trail.
#[derive(Debug, Clone, Serialize)]
pub struct CallRecord {
    pub ts_unix_ms: u64,
    pub meta_tool: MetaTool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub latency_ms: u64,
    pub outcome: CallOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
    pub arg_bytes: usize,
    pub result_bytes: usize,
}

impl CallRecord {
    /// Current unix time in milliseconds (for `ts_unix_ms`).
    pub fn now_unix_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A sink for call observations. Implementations MUST be non-blocking and MUST NOT panic — an
/// observation failure must never affect the tool call itself.
pub trait CallSink: Send + Sync {
    fn record(&self, rec: &CallRecord);
}

/// T1 sink: emit each record as a structured `tracing` event (reusing the process subscriber).
pub struct TracingSink;

impl CallSink for TracingSink {
    fn record(&self, r: &CallRecord) {
        tracing::info!(
            meta_tool = r.meta_tool.as_str(),
            target_tool = r.target_tool.as_deref(),
            upstream = r.upstream.as_deref(),
            latency_ms = r.latency_ms,
            outcome = r.outcome.as_str(),
            error_kind = r.error_kind,
            arg_bytes = r.arg_bytes,
            result_bytes = r.result_bytes,
            "tool_call"
        );
    }
}

#[cfg(feature = "testkit")]
mod capture {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Test sink that captures every record for assertions.
    #[derive(Clone, Default)]
    pub struct CaptureSink {
        records: Arc<Mutex<Vec<CallRecord>>>,
    }
    impl CaptureSink {
        pub fn new() -> Self {
            Self::default()
        }
        /// Snapshot of all records seen so far.
        pub fn records(&self) -> Vec<CallRecord> {
            self.records.lock().unwrap().clone()
        }
    }
    impl CallSink for CaptureSink {
        fn record(&self, rec: &CallRecord) {
            self.records.lock().unwrap().push(rec.clone());
        }
    }
}
#[cfg(feature = "testkit")]
pub use capture::CaptureSink;

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn serializes_enums_as_snake_case_short_strings() {
        let v: serde_json::Value = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["meta_tool"], "call_tool");
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["target_tool"], "github__create_issue");
        assert_eq!(v["arg_bytes"], 42);
        // as_str() must match the serde token exactly (both sinks agree on spelling).
        assert_eq!(MetaTool::CallTool.as_str(), "call_tool");
        assert_eq!(CallOutcome::Ok.as_str(), "ok");
    }

    #[test]
    fn record_is_metadata_only_exact_key_set() {
        // The TYPE cannot carry argument/result content; lock the serialized key set to EXACTLY
        // the allowed metadata keys (no "arguments"/"args"/"result"/"content"/"text"). Populate
        // every Option so all keys serialize; exact equality means any added field must be
        // deliberately acknowledged here.
        let mut r = sample();
        r.error_kind = Some("timeout");
        let v = serde_json::to_value(r).unwrap();
        let keys: std::collections::HashSet<String> =
            v.as_object().unwrap().keys().cloned().collect();
        let allowed: std::collections::HashSet<String> = [
            "ts_unix_ms",
            "meta_tool",
            "target_tool",
            "upstream",
            "latency_ms",
            "outcome",
            "error_kind",
            "arg_bytes",
            "result_bytes",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            keys, allowed,
            "serialized key set must be exactly the metadata fields"
        );
    }

    #[test]
    fn skips_none_optionals() {
        let mut r = sample();
        r.target_tool = None;
        r.upstream = None;
        r.error_kind = None;
        let v = serde_json::to_value(r).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("target_tool"));
        assert!(!obj.contains_key("upstream"));
        assert!(!obj.contains_key("error_kind"));
    }
}
