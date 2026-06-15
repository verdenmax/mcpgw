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

/// Removes the given paths on drop, so a panicking assertion can't leak temp files.
struct TempCleanup(Vec<std::path::PathBuf>);
impl Drop for TempCleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[tokio::test]
async fn serve_with_audit_enabled_writes_jsonl_for_a_meta_tool_call() {
    let audit_path = unique_temp("audit-it.jsonl");
    let cfg_path = unique_temp("audit-it-config.toml");
    let _cleanup = TempCleanup(vec![audit_path.clone(), cfg_path.clone()]);
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

    // Spawn `mcpgw serve --config <cfg>` and connect an MCP client over its stdio. Use the builder
    // (not `::new`) so stderr is actually suppressed — `::new` re-applies the inherit default and
    // would let the child's logs through.
    let (transport, _stderr) = TokioChildProcess::builder(Command::new(bin()).configure(|c| {
        c.arg("serve").arg("--config").arg(&cfg_path);
    }))
    .stderr(Stdio::null())
    .spawn()
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
    // Positive metadata assertions (load-bearing line-shape check) ...
    assert_eq!(v["meta_tool"], "search_tools");
    assert_eq!(v["outcome"], "ok");
    assert!(
        v.get("ts_unix_ms").is_some() && v.get("arg_bytes").is_some(),
        "audit line must carry metadata fields: {v}"
    );
    // ... and the metadata-only invariant: no argument/result payload.
    assert!(
        v.get("arguments").is_none(),
        "audit line must not contain payloads"
    );
}
