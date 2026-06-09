//! mcpgw `upstream`: connect to upstream MCP servers, ingest their tools into a
//! namespaced catalog, and forward tool calls.

pub mod connection;
pub mod mapping;
pub mod registry;
pub mod testkit;

#[cfg(test)]
mod spike_tests {
    use super::testkit::MockUpstream;
    use rmcp::ServiceExt;

    /// Spike: stand up the mock upstream as an rmcp server over an in-memory duplex,
    /// connect a client, and confirm the client sees the mock's two tools.
    #[tokio::test]
    async fn client_lists_mock_upstream_tools_over_duplex() {
        let (server_io, client_io) = tokio::io::duplex(4096);

        let server = tokio::spawn(async move {
            let svc = MockUpstream::new().serve(server_io).await.unwrap();
            svc.waiting().await.unwrap();
        });

        let client = ().serve(client_io).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(names.contains(&"echo".to_string()), "tools were: {names:?}");
        assert!(
            names.contains(&"greet".to_string()),
            "tools were: {names:?}"
        );

        client.cancel().await.unwrap();
        server.abort();
    }
}
