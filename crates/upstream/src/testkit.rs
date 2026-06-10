//! In-memory mock upstream MCP server for tests. Reusable by other crates via the
//! `testkit` feature. Exposes two tools: `echo` and `greet`.
#![cfg(any(test, feature = "testkit"))]

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EchoParams {
    /// Text to echo back.
    pub text: String,
}

#[derive(Clone)]
pub struct MockUpstream {
    tool_router: ToolRouter<MockUpstream>,
}

#[tool_router]
impl MockUpstream {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Echo the provided text back")]
    fn echo(
        &self,
        Parameters(EchoParams { text }): Parameters<EchoParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Greet the world")]
    fn greet(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::success(vec![Content::text("hello")]))
    }

    #[tool(description = "Sleep 10s then return (for timeout tests)")]
    async fn slow(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        Ok(CallToolResult::success(vec![Content::text("done")]))
    }
}

impl Default for MockUpstream {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MockUpstream {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }
}
