//! A live connection to one upstream MCP server.

use catalog::Catalog;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{NotificationContext, RoleClient, RunningService};
use rmcp::transport::IntoTransport;
use rmcp::{ClientHandler, ServiceExt};

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

/// A bounded channel the gateway drains to rebuild its snapshot. The handler sends the
/// upstream's name on each `tools/list_changed`; a full channel is fine (worker coalesces).
pub type RebuildTrigger = tokio::sync::mpsc::Sender<String>;

/// rmcp client handler installed on every upstream connection. On `tools/list_changed`
/// it nudges the rebuild trigger; with `trigger: None` it is a no-op (used by in-memory tests).
#[derive(Clone)]
pub struct UpstreamClientHandler {
    server: String,
    trigger: Option<RebuildTrigger>,
}

impl ClientHandler for UpstreamClientHandler {
    async fn on_tool_list_changed(&self, _ctx: NotificationContext<RoleClient>) {
        if let Some(tx) = &self.trigger {
            let _ = tx.try_send(self.server.clone());
        }
    }
}

/// A connected upstream MCP server: its namespace name + the running rmcp client.
pub struct UpstreamHandle {
    server: String,
    client: RunningService<RoleClient, UpstreamClientHandler>,
    call_timeout: std::time::Duration,
}

impl UpstreamHandle {
    /// Connect over any transport with NO list_changed trigger (in-memory tests).
    pub async fn connect<T, E, A>(server: &str, transport: T) -> Result<Self, UpstreamError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::connect_with_trigger(server, transport, None).await
    }

    /// Connect and install an `UpstreamClientHandler` carrying `trigger`.
    pub async fn connect_with_trigger<T, E, A>(
        server: &str,
        transport: T,
        trigger: Option<RebuildTrigger>,
    ) -> Result<Self, UpstreamError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let handler = UpstreamClientHandler {
            server: server.to_string(),
            trigger,
        };
        let client = handler
            .serve(transport)
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

    /// Cancel the underlying rmcp service via its cancellation token, WITHOUT consuming the
    /// handle. Unlike `shutdown(self)` — which awaits `client.cancel()` (drain + transport close) —
    /// this is **fire-and-forget**: it signals cancellation and returns immediately without
    /// awaiting cleanup. It works on a shared `&self` (e.g. an `Arc<UpstreamHandle>` still held by
    /// the rebuild worker or an in-flight call), so teardown never silently skips a cancel.
    /// Signalling the token stops the service loop, which closes the transport and (for
    /// child-process upstreams) reaps the child — independent of when the last `Arc` clone drops.
    pub fn cancel(&self) {
        self.client.cancellation_token().cancel();
    }
}
