//! End-to-end: `mcpgw serve` with the dashboard enabled serves /api/* and captures a discovery
//! trace for a search_tools call. Ignored by default (binds a TCP port), run with `--ignored`.

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

#[tokio::test]
#[ignore = "binds a TCP port; run with --ignored"]
async fn dashboard_serves_api_and_captures_a_trace() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let port = 20000 + (nanos % 20000) as u16;
    let cfg_path = std::env::temp_dir().join(format!("mcpgw-dash-{nanos}.toml"));
    {
        let mut f = std::fs::File::create(&cfg_path).unwrap();
        writeln!(
            f,
            "[server]\nstdio = true\n\n[dashboard]\nenabled = true\nbind = \"127.0.0.1:{port}\"\ntrace_queries = true\n"
        )
        .unwrap();
    }

    let (transport, _stderr) = TokioChildProcess::builder(Command::new(bin()).configure(|c| {
        c.arg("serve").arg("--config").arg(&cfg_path);
    }))
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let client = ().serve(transport).await.unwrap();

    // Drive a search so a discovery trace is captured (empty catalog -> empty results, still traced).
    let _ = client
        .call_tool(
            CallToolRequestParams::new("search_tools").with_arguments(
                json!({ "query": "weather forecast" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();

    let ov: serde_json::Value = http
        .get(format!("{base}/api/overview"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ov["strategy"], "bm25");

    let traces: serde_json::Value = http
        .get(format!("{base}/api/traces?source=live"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = traces["traces"].as_array().unwrap();
    assert!(
        arr.iter().any(|t| t["query"] == "weather forecast"),
        "the search query was captured"
    );

    // M1: the search above produced one CallRecord captured by the live CallRingSink.
    let calls: serde_json::Value = http
        .get(format!("{base}/api/calls?source=live"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        calls["total"].as_u64().unwrap() >= 1,
        "the search_tools call was captured"
    );
    let items = calls["items"].as_array().unwrap();
    assert_eq!(items[0]["meta_tool"], "search_tools");
    let id = items[0]["id"].as_str().unwrap().to_string();

    // Detail by live id resolves to the same call.
    let detail_resp = http
        .get(format!("{base}/api/calls/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(detail_resp.status(), 200);
    let detail: serde_json::Value = detail_resp.json().await.unwrap();
    assert_eq!(detail["meta_tool"], "search_tools");

    // Unknown live id -> 404.
    let missing = http
        .get(format!("{base}/api/calls/999999"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // M2: `/` serves the embedded Svelte app (text/html with the mount point).
    let root = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(root.status(), 200);
    let ctype = root
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ctype.starts_with("text/html"), "/ is HTML, got {ctype}");
    let body = root.text().await.unwrap();
    assert!(body.contains("id=\"app\""), "/ returns the SPA mount point");

    // M3: trace detail happy-path (the search above created a live trace with an id).
    let tr: serde_json::Value = http
        .get(format!("{base}/api/traces?source=live&limit=10"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tid = tr["traces"][0]["id"]
        .as_str()
        .expect("a live trace id")
        .to_string();
    let td = http
        .get(format!("{base}/api/traces/{tid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(td.status(), 200);
    let tdj: serde_json::Value = td.json().await.unwrap();
    assert_eq!(tdj["query"], "weather forecast");

    // M3: unknown upstream/tool/trace detail -> 404 (this config has no upstreams / empty catalog).
    assert_eq!(
        http.get(format!("{base}/api/upstreams/nope"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.get(format!("{base}/api/tools/nope__missing"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.get(format!("{base}/api/traces/h9-9"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    client.cancel().await.unwrap();
    let _ = std::fs::remove_file(&cfg_path);
}

#[tokio::test]
#[ignore = "binds a TCP port + spawns mock-stdio; run with --ignored (and --all-features to build mock-stdio)"]
async fn dashboard_detail_endpoints_with_mock_upstream() {
    // mock-stdio is a sibling binary in the same target dir as the mcpgw test binary.
    let mock = std::path::Path::new(env!("CARGO_BIN_EXE_mcpgw"))
        .parent()
        .unwrap()
        .join("mock-stdio");
    if !mock.exists() {
        eprintln!("skip: mock-stdio not built (run with --all-features); looked at {mock:?}");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let port = 20000 + (nanos % 20000) as u16;
    let cfg_path = std::env::temp_dir().join(format!("mcpgw-m3-{nanos}.toml"));
    {
        let mut f = std::fs::File::create(&cfg_path).unwrap();
        writeln!(
            f,
            "[server]\nstdio = true\n\n[dashboard]\nenabled = true\nbind = \"127.0.0.1:{port}\"\ntrace_queries = true\n\n[[upstream]]\nname = \"mock\"\ntransport = \"stdio\"\ncommand = {mock:?}\nargs = []\ncall_timeout_ms = 30000\n"
        )
        .unwrap();
    }

    let (transport, _stderr) = TokioChildProcess::builder(Command::new(bin()).configure(|c| {
        c.arg("serve").arg("--config").arg(&cfg_path);
    }))
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let client = ().serve(transport).await.unwrap();

    // Drive a search (creates a trace) + a real call (so the upstream has call metrics).
    let _ = client
        .call_tool(
            CallToolRequestParams::new("search_tools").with_arguments(
                json!({ "query": "echo greet" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(
                json!({ "name": "mock__echo", "arguments": { "text": "hi" } })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();

    // Upstream detail happy-path: the mock upstream exposes 4 tools.
    let ud: serde_json::Value = http
        .get(format!("{base}/api/upstreams/mock"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ud["name"], "mock");
    assert_eq!(
        ud["tools_count"].as_u64().unwrap(),
        4,
        "echo/greet/slow/fail"
    );
    let tool_names: Vec<&str> = ud["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tool_names.contains(&"mock__echo"), "tools: {tool_names:?}");

    // Tool detail happy-path: schema + owning server.
    let toolj = http
        .get(format!("{base}/api/tools/mock__echo"))
        .send()
        .await
        .unwrap();
    assert_eq!(toolj.status(), 200);
    let toolj: serde_json::Value = toolj.json().await.unwrap();
    assert_eq!(toolj["name"], "mock__echo");
    assert_eq!(toolj["server"], "mock");
    assert!(toolj.get("input_schema").is_some());

    // Trace detail happy-path via a list-assigned id.
    let tr: serde_json::Value = http
        .get(format!("{base}/api/traces?source=live&limit=10"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tid = tr["traces"][0]["id"]
        .as_str()
        .expect("trace id")
        .to_string();
    assert_eq!(
        http.get(format!("{base}/api/traces/{tid}"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    client.cancel().await.unwrap();
    let _ = std::fs::remove_file(&cfg_path);
}
