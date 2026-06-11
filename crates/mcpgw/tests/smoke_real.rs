//! Real end-to-end smoke test (gated): spawn the actual `mcpgw` binary, which spawns the
//! official reference MCP server `@modelcontextprotocol/server-everything` as a real stdio
//! upstream, and drive the gateway through both downstream transports.
//!
//! These tests are `#[ignore]`d: they require network access (first `npx` run downloads the
//! package), Node.js/`npx` on PATH, and they spawn real child processes. Run explicitly with:
//!
//! ```bash
//! cargo test -p mcpgw --test smoke_real -- --ignored --nocapture
//! ```
//!
//! Topology: test (rmcp client) → mcpgw serve → server-everything (npx child).
//!
//! NOTE: the upstream `env_passthrough` MUST include `PATH` (and `HOME`) — mcpgw clears the
//! child's environment and only re-injects the allow-listed vars, so without `PATH` the
//! `npx`/`node` child cannot resolve its own dependencies.

use std::path::PathBuf;

use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::json;

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

/// Write a temp config whose only upstream is the real `server-everything` (stdio). `server`
/// is the body of the `[server]` table plus any extra sections (e.g. `[server.http]`).
fn write_config(server: &str, tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpgw_smoke_{}_{tag}.toml", std::process::id()));
    let toml = format!(
        "[server]\n{server}\n\n\
         [[upstream]]\n\
         name = \"everything\"\n\
         transport = \"stdio\"\n\
         command = \"npx\"\n\
         args = [\"-y\", \"@modelcontextprotocol/server-everything\", \"stdio\"]\n\
         env_passthrough = [\"PATH\", \"HOME\"]\n"
    );
    std::fs::write(&path, toml).unwrap();
    path
}

#[tokio::test]
#[ignore = "real e2e: needs npx/node + network (first run downloads the package)"]
async fn smoke_stdio_real_everything_upstream() {
    let cfg = write_config("stdio = true", "stdio");
    let bin = env!("CARGO_BIN_EXE_mcpgw");

    // Spawn the real mcpgw binary and speak MCP to it over its stdio.
    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|c| {
        c.arg("--config").arg(&cfg).arg("serve");
    }))
    .expect("spawn mcpgw");
    let client = ().serve(transport).await.expect("connect to mcpgw over stdio");

    // Downstream exposes exactly the 3 meta-tools, regardless of upstream tools.
    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["call_tool", "get_tool_details", "search_tools"]);

    // search_tools surfaces the real upstream's namespaced echo tool.
    let r = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query": "echo message", "top_k": 20}))),
        )
        .await
        .unwrap();
    let found = r.content[0].as_text().unwrap().text.clone();
    assert!(found.contains("everything__echo"), "search result: {found}");

    // get_tool_details returns the real input schema (echo requires `message`).
    let r = client
        .call_tool(
            CallToolRequestParams::new("get_tool_details")
                .with_arguments(args(json!({"name": "everything__echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("message"));

    // call_tool forwards to the real upstream and returns its echoed payload.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "everything__echo",
                "arguments": {"message": "hi-smoke"}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("hi-smoke"));

    // A hyphenated upstream tool name exercises the "never split on __" routing invariant.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "everything__get-sum",
                "arguments": {"a": 2, "b": 3}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains('5'));

    client.cancel().await.unwrap();
    let _ = std::fs::remove_file(&cfg);
}

#[tokio::test]
#[ignore = "real e2e: needs npx/node + network (first run downloads the package)"]
async fn smoke_http_real_everything_upstream() {
    // Pick a free port, then hand it to mcpgw (small TOCTOU window, acceptable for a smoke test).
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cfg = write_config(
        &format!("stdio = false\n\n[server.http]\nenabled = true\nbind = \"127.0.0.1:{port}\""),
        "http",
    );
    let bin = env!("CARGO_BIN_EXE_mcpgw");

    // Run mcpgw as a daemon (HTTP only); stderr (logs) inherits the test's stderr.
    let mut mcpgw = tokio::process::Command::new(bin)
        .arg("--config")
        .arg(&cfg)
        .arg("serve")
        .spawn()
        .expect("spawn mcpgw http daemon");

    // mcpgw connects upstreams + builds the snapshot BEFORE binding HTTP, so wait for the port.
    let mut bound = false;
    for _ in 0..150 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            bound = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(bound, "mcpgw HTTP server never bound on 127.0.0.1:{port}");

    let url = format!("http://127.0.0.1:{port}/mcp");
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(url))
            .await
            .expect("connect to mcpgw over HTTP");

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 3);

    // Full search → call over HTTP against the real upstream.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "everything__get-sum",
                "arguments": {"a": 40, "b": 2}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("42"));

    client.cancel().await.unwrap();
    let _ = mcpgw.start_kill();
    let _ = std::fs::remove_file(&cfg);
}
