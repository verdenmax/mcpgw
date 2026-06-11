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

pub mod http;

/// The downstream MCP server. Holds shared gateway state plus the default `top_k`
/// used when a `search_tools` call omits it (sourced from `[retrieval].top_k`).
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<GatewayState>,
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
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let args = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "search_tools" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let top_k = args
                    .get("top_k")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(self.default_top_k);
                let snap = self.state.snapshot();
                let hits = metatools::search_tools(&snap, query, top_k);
                let json = serde_json::to_string(&hits)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            "get_tool_details" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let snap = self.state.snapshot();
                match metatools::get_tool_details(&snap, name) {
                    Some(def) => {
                        let json = serde_json::to_string(def)
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    None => Ok(CallToolResult::error(vec![Content::text(format!(
                        "no such tool: {name}"
                    ))])),
                }
            }
            "call_tool" => {
                let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "missing required 'name'",
                    )]));
                };
                let inner = args.get("arguments").and_then(|v| v.as_object()).cloned();
                let snap = self.state.snapshot();
                match metatools::call_tool(&snap, self.state.registry(), name, inner).await {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
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
