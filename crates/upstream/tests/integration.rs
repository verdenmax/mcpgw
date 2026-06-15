use std::sync::Arc;

use catalog::Catalog;
use upstream::connection::UpstreamHandle;
use upstream::registry::UpstreamRegistry;
use upstream::testkit::MockUpstream;

use rmcp::ServiceExt;

/// Spawn the mock upstream over a duplex and return a connected UpstreamHandle.
async fn connect_mock(name: &str) -> (UpstreamHandle, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    (handle, server)
}

#[tokio::test]
async fn ingests_namespaced_tools_from_upstream() {
    let (handle, server) = connect_mock("mock").await;
    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();

    assert_eq!(catalog.len(), 4);
    assert!(catalog.get("mock__echo").is_some());
    assert!(catalog.get("mock__greet").is_some());
    assert!(catalog.get("mock__slow").is_some());
    assert!(catalog.get("mock__fail").is_some());

    handle.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn forwards_call_tool_to_upstream() {
    let (handle, server) = connect_mock("mock").await;

    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("ping".into()));
    let result = handle.call_tool("echo", Some(args)).await.unwrap();

    // CallToolResult::success sets is_error = Some(false); assert it exactly.
    assert_eq!(result.is_error, Some(false));

    // The mock's echo returns the text as a text content block.
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .expect("echo should return a text content block");
    assert_eq!(text, "ping");

    handle.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn registry_returns_handle_by_name_and_none_for_missing() {
    let (handle, server) = connect_mock("mock").await;
    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(handle));

    assert_eq!(registry.server_names(), vec!["mock".to_string()]);
    assert!(registry.get("mock").is_some());
    assert!(registry.get("nope").is_none());

    // Forward a call through the registry-held handle.
    let h = registry.get("mock").unwrap();
    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("x".into()));
    let r = h.call_tool("echo", Some(args)).await.unwrap();
    assert_eq!(r.is_error, Some(false));

    server.abort();
}

#[tokio::test]
async fn registry_remove_returns_handle_and_clears_entry() {
    let (handle, server) = connect_mock("mock").await;
    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(handle));

    let removed = registry.remove("mock");
    assert!(removed.is_some(), "remove should return the handle");
    assert!(registry.get("mock").is_none());
    assert!(registry.server_names().is_empty());
    assert!(
        registry.remove("mock").is_none(),
        "second remove is a no-op"
    );

    server.abort();
}

#[tokio::test]
async fn one_upstream_failure_does_not_block_others() {
    // Healthy upstream:
    let (good, good_server) = connect_mock("good").await;

    // "Hung" upstream: keep the server end of the duplex ALIVE but never serve on it,
    // so the client's `initialize` request is never answered and `connect` blocks until
    // the timeout fires. (Dropping the server end would instead fail fast with EOF; the
    // timeout is what makes a *hung* peer non-blocking for the rest of the gateway.)
    let (_hung_server_io, hung_client_io) = tokio::io::duplex(4096);
    let bad = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        UpstreamHandle::connect("bad", hung_client_io),
    )
    .await;
    // Self-validate the failure injection: the hung connect must NOT have succeeded.
    assert!(
        !matches!(bad, Ok(Ok(_))),
        "the hung upstream should not have connected"
    );

    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(good));
    if let Ok(Ok(h)) = bad {
        registry.insert(Arc::new(h));
    }
    // The failed upstream must be absent; only the healthy one is registered.
    assert_eq!(registry.server_names(), vec!["good".to_string()]);

    // The healthy upstream remains fully usable despite the hung peer.
    let mut catalog = Catalog::new();
    registry
        .get("good")
        .unwrap()
        .ingest_into(&mut catalog)
        .await
        .unwrap();
    assert!(catalog.get("good__echo").is_some());
    assert!(catalog.get("good__greet").is_some());

    good_server.abort();
}

#[tokio::test]
async fn call_tool_times_out_when_slower_than_call_timeout() {
    let (handle, server) = connect_mock("mock").await;
    let handle = handle.with_call_timeout(std::time::Duration::from_millis(50));

    let err = handle.call_tool("slow", None).await.unwrap_err();
    assert!(
        matches!(err, upstream::connection::UpstreamError::Timeout { .. }),
        "expected Timeout, got {err:?}"
    );

    server.abort();
}

#[tokio::test]
async fn connect_with_trigger_preserves_ingest_and_call() {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        MockUpstream::new()
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
    let handle = UpstreamHandle::connect_with_trigger("mock", client_io, Some(tx))
        .await
        .unwrap();

    let mut cat = Catalog::new();
    handle.ingest_into(&mut cat).await.unwrap();
    assert!(cat.get("mock__echo").is_some());

    let r = handle
        .call_tool(
            "echo",
            serde_json::json!({"text":"hi"}).as_object().cloned(),
        )
        .await
        .unwrap();
    assert!(r.content[0].as_text().unwrap().text.contains("hi"));

    // No list_changed occurred, so the trigger channel must be empty.
    assert!(rx.try_recv().is_err());
}

use config::{UpstreamConfig, UpstreamTransport};
use upstream::connect::{connect_all, connect_stdio_upstream};

fn stdio_cfg(name: &str, command: &str, args: Vec<String>) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        call_timeout_ms: 5_000,
        transport: UpstreamTransport::Stdio {
            command: command.to_string(),
            args,
            env_passthrough: vec![],
        },
    }
}

#[tokio::test]
async fn connect_all_degraded_start_isolates_bad_upstreams() {
    let registry = UpstreamRegistry::new();
    let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
    let cfgs = vec![
        stdio_cfg("bad1", "definitely-not-a-real-binary-xyzzy", vec![]),
        stdio_cfg("bad2", "definitely-not-a-real-binary-zzz", vec![]),
    ];
    let summary = connect_all(&registry, &cfgs, tx).await;
    assert!(summary.connected.is_empty());
    assert_eq!(summary.skipped.len(), 2);
    assert!(registry.server_names().is_empty());
}

#[tokio::test]
async fn connect_stdio_upstream_smoke_spawns_real_child() {
    let exe = env!("CARGO_BIN_EXE_mock-stdio");
    let cfg = stdio_cfg("child", exe, vec![]);
    let handle = connect_stdio_upstream(&cfg, None)
        .await
        .expect("spawn + connect");

    let mut cat = catalog::Catalog::new();
    handle.ingest_into(&mut cat).await.unwrap();
    assert!(cat.get("child__echo").is_some());

    std::sync::Arc::new(handle); // drop cancels the child service
}

use upstream::testkit::RevealingMockUpstream;

#[tokio::test]
async fn revealing_mock_grows_its_tool_list_after_reveal() {
    use rmcp::model::CallToolRequestParams;
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        RevealingMockUpstream::new()
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();

    let before: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(before.contains(&"echo".to_string()));
    assert!(before.contains(&"reveal".to_string()));
    assert!(!before.contains(&"late_tool".to_string()));

    client
        .call_tool(CallToolRequestParams::new("reveal"))
        .await
        .unwrap();

    let after: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(
        after.contains(&"late_tool".to_string()),
        "reveal must expose late_tool: {after:?}"
    );

    client.cancel().await.unwrap();
}
