//! A live connection to one upstream MCP server.

use catalog::Catalog;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::mapping::ingest_tools;

/// Errors from upstream operations.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("failed to connect to upstream {server:?}: {source}")]
    Connect {
        server: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("upstream {server:?} call failed: {source}")]
    Call {
        server: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("upstream {server:?} call timed out")]
    Timeout { server: String },
}

/// A connected upstream MCP server: its namespace name + the running rmcp client.
pub struct UpstreamHandle {
    server: String,
    client: RunningService<RoleClient, ()>,
    call_timeout: std::time::Duration,
}

impl UpstreamHandle {
    /// Connect over any async-rw transport (a real stdio child or an in-memory duplex).
    pub async fn connect<T>(server: &str, transport: T) -> Result<Self, UpstreamError>
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let client =
            ().serve(transport)
                .await
                .map_err(|e| UpstreamError::Connect {
                    server: server.to_string(),
                    source: Box::new(e),
                })?;
        Ok(Self {
            server: server.to_string(),
            client,
            call_timeout: std::time::Duration::from_secs(30),
        })
    }

    /// Set the per-call timeout (consumed before the handle is shared via `Arc`).
    pub fn with_call_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// The per-call timeout configured for this handle (used by the gateway to bound ingest).
    pub fn call_timeout(&self) -> std::time::Duration {
        self.call_timeout
    }

    /// Fetch this server's tools and ingest them (namespaced) into `catalog`.
    /// Returns the number of intra-server duplicate tool names that were skipped
    /// (also `warn!`-logged), so callers can surface ingest telemetry.
    pub async fn ingest_into(&self, catalog: &mut Catalog) -> Result<usize, UpstreamError> {
        let tools = self
            .client
            .list_all_tools()
            .await
            .map_err(|e| UpstreamError::Call {
                server: self.server.clone(),
                source: Box::new(e),
            })?;
        Ok(ingest_tools(catalog, &self.server, &tools))
    }

    /// Forward a tool call to this upstream. `tool` is the ORIGINAL (un-namespaced) name.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult, UpstreamError> {
        let mut params = CallToolRequestParams::new(tool.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }
        let fut = self.client.call_tool(params);
        match tokio::time::timeout(self.call_timeout, fut).await {
            Err(_elapsed) => Err(UpstreamError::Timeout {
                server: self.server.clone(),
            }),
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(UpstreamError::Call {
                server: self.server.clone(),
                source: Box::new(e),
            }),
        }
    }

    /// Cancel the underlying rmcp service.
    pub async fn shutdown(self) {
        let _ = self.client.cancel().await;
    }
}
