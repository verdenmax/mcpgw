use catalog::Catalog;
use upstream::connection::UpstreamHandle;
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

    assert_eq!(catalog.len(), 2);
    assert!(catalog.get("mock__echo").is_some());
    assert!(catalog.get("mock__greet").is_some());

    handle.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn forwards_call_tool_to_upstream() {
    let (handle, server) = connect_mock("mock").await;

    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("ping".into()));
    let result = handle.call_tool("echo", Some(args)).await.unwrap();

    // The mock's echo returns the text as a text content block.
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert_eq!(text, "ping");
    assert_ne!(result.is_error, Some(true));

    handle.shutdown().await;
    server.abort();
}
