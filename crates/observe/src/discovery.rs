//! Optional, opt-in capture of search/discovery traces (query -> selected tools + scores).
//! Kept SEPARATE from the metadata-only `CallRecord` so query text never leaks into the
//! privacy-clean call sinks (tracing/audit).

use serde::Serialize;

/// One returned tool in a discovery trace: its namespaced name and relevance score.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveryHit {
    pub name: String,
    pub score: f32,
}

/// One `search_tools` call: the raw query and the tools it surfaced (with scores).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveryRecord {
    pub ts_unix_ms: u64,
    pub query: String,
    pub top_k: usize,
    pub results: Vec<DiscoveryHit>,
    pub latency_ms: u64,
}

/// Fan-out target for discovery traces. Implemented by the dashboard's in-memory ring buffer.
pub trait DiscoverySink: Send + Sync {
    fn record(&self, rec: &DiscoveryRecord);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn discovery_record_serializes_with_expected_keys() {
        let rec = DiscoveryRecord {
            ts_unix_ms: 1,
            query: "weather".into(),
            top_k: 2,
            results: vec![DiscoveryHit {
                name: "w__get".into(),
                score: 1.5,
            }],
            latency_ms: 3,
        };
        let v: serde_json::Value = serde_json::to_value(&rec).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<_> = obj.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["latency_ms", "query", "results", "top_k", "ts_unix_ms"]
        );
        assert_eq!(obj["results"][0]["name"], "w__get");
        assert_eq!(obj["results"][0]["score"], 1.5);
    }

    #[test]
    fn discovery_sink_receives_records() {
        struct Collect(Mutex<Vec<DiscoveryRecord>>);
        impl DiscoverySink for Collect {
            fn record(&self, rec: &DiscoveryRecord) {
                self.0.lock().unwrap().push(rec.clone());
            }
        }
        let sink = Collect(Mutex::new(Vec::new()));
        let rec = DiscoveryRecord {
            ts_unix_ms: 0,
            query: "q".into(),
            top_k: 1,
            results: vec![],
            latency_ms: 0,
        };
        sink.record(&rec);
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }
}
