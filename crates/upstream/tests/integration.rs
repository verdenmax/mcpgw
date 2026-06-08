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
