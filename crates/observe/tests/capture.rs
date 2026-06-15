#![cfg(feature = "testkit")]
use observe::{CallOutcome, CallRecord, CallSink, CaptureSink, MetaTool};

fn rec(o: CallOutcome) -> CallRecord {
    CallRecord {
        ts_unix_ms: 0,
        meta_tool: MetaTool::SearchTools,
        target_tool: None,
        upstream: None,
        latency_ms: 0,
        outcome: o,
        error_kind: None,
        arg_bytes: 0,
        result_bytes: 0,
    }
}

#[test]
fn capture_sink_records_in_order() {
    let s = CaptureSink::new();
    s.record(&rec(CallOutcome::Ok));
    s.record(&rec(CallOutcome::Error));
    let got = s.records();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].outcome, CallOutcome::Ok);
    assert_eq!(got[1].outcome, CallOutcome::Error);
}
