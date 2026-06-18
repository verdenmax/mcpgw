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

    client.cancel().await.unwrap();
    let _ = std::fs::remove_file(&cfg_path);
}
