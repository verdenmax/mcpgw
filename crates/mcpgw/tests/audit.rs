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
