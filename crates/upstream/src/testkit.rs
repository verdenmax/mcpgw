//! In-memory mock upstream MCP servers for tests. Reusable by other crates via the
//! `testkit` feature. `MockUpstream` exposes three static tools: `echo`, `greet`, and
//! `slow` (sleeps to exercise per-call timeouts). `RevealingMockUpstream` is a
//! runtime-revealing mock whose tool list grows when `reveal` is called (emitting
//! `tools/list_changed`), used to drive the gateway's list_changed refresh end-to-end.
#![cfg(any(test, feature = "testkit"))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler};

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

fn empty_object_schema() -> Arc<serde_json::Map<String, serde_json::Value>> {
    match serde_json::json!({"type": "object"}) {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => Arc::new(serde_json::Map::new()),
    }
}

/// A mock upstream whose tool list changes at runtime: it starts with `echo` + `reveal`,
/// and calling `reveal` exposes `late_tool` AND emits `tools/list_changed` to the client.
/// Used to drive the gateway's list_changed refresh path end-to-end.
#[derive(Clone, Default)]
pub struct RevealingMockUpstream {
    revealed: Arc<AtomicBool>,
}

impl RevealingMockUpstream {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ServerHandler for RevealingMockUpstream {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::from_build_env())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = vec![
            Tool::new("echo", "Echo the input text", empty_object_schema()),
            Tool::new(
                "reveal",
                "Reveal late_tool and emit tools/list_changed",
                empty_object_schema(),
            ),
        ];
        if self.revealed.load(Ordering::SeqCst) {
            tools.push(Tool::new(
                "late_tool",
                "A tool revealed at runtime",
                empty_object_schema(),
            ));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match request.name.as_ref() {
            "echo" => {
                let text = request
                    .arguments
                    .as_ref()
                    .and_then(|a| a.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            "reveal" => {
                self.revealed.store(true, Ordering::SeqCst);
                let _ = ctx.peer.notify_tool_list_changed().await;
                Ok(CallToolResult::success(vec![Content::text("revealed")]))
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
}
