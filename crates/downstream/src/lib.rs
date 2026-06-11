//! mcpgw `downstream`: a rmcp `ServerHandler` that exposes the gateway's 3 meta-tools
//! (`search_tools` / `get_tool_details` / `call_tool`) to MCP clients over stdio.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use gateway::GatewayState;

/// The downstream MCP server. Holds shared gateway state plus the default `top_k`
/// used when a `search_tools` call omits it (sourced from `[retrieval].top_k`).
#[derive(Clone)]
pub struct GatewayServer {
    // Read by `call_tool` dispatch (added in Task 3); allow until then.
    #[allow(dead_code)]
    state: Arc<GatewayState>,
    #[allow(dead_code)]
    default_top_k: usize,
}

impl GatewayServer {
    pub fn new(state: Arc<GatewayState>, default_top_k: usize) -> Self {
        Self {
            state,
            default_top_k,
        }
    }
}

fn object_schema(json: serde_json::Value) -> Arc<serde_json::Map<String, serde_json::Value>> {
    match json {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => Arc::new(serde_json::Map::new()),
    }
}

/// The fixed set of 3 meta-tools exposed to clients. Stable regardless of upstreams.
pub fn meta_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "search_tools",
            "Search aggregated upstream tools by natural-language query; returns candidate \
             tool summaries (qualified name + description).",
            object_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural-language query." },
                    "top_k": { "type": "integer", "description": "Max results to return." }
                },
                "required": ["query"]
            })),
        ),
        Tool::new(
            "get_tool_details",
            "Get the full definition (description + input schema) of one tool by its \
             qualified name (e.g. \"github__create_issue\").",
            object_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Qualified tool name." }
                },
                "required": ["name"]
            })),
        ),
        Tool::new(
            "call_tool",
            "Execute one upstream tool by its qualified name, forwarding `arguments`.",
            object_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Qualified tool name." },
                    "arguments": { "type": "object", "description": "Tool arguments." }
                },
                "required": ["name"]
            })),
        ),
    ]
}

impl ServerHandler for GatewayServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(meta_tools()))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Dispatch is implemented in a later task; placeholder so the skeleton compiles.
        Ok(CallToolResult::error(vec![Content::text(
            "not implemented",
        )]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_tools_are_exactly_the_three_with_schemas() {
        let tools = meta_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, ["search_tools", "get_tool_details", "call_tool"]);

        // search_tools must have query (required) + top_k (optional).
        let search = &tools[0];
        let props = search.input_schema.get("properties").unwrap();
        assert!(props.get("query").is_some());
        assert!(props.get("top_k").is_some());
        let required = search.input_schema.get("required").unwrap();
        assert_eq!(required, &serde_json::json!(["query"]));
    }
}
