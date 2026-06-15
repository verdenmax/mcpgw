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

/// The downstream MCP server. Holds shared gateway state, the default `top_k`, and the
/// observation sinks each meta-tool call is reported to.
#[derive(Clone)]
pub struct GatewayServer {
    state: Arc<GatewayState>,
    default_top_k: usize,
    sinks: Arc<[Arc<dyn observe::CallSink>]>,
}

impl GatewayServer {
    pub fn new(
        state: Arc<GatewayState>,
        default_top_k: usize,
        sinks: Arc<[Arc<dyn observe::CallSink>]>,
    ) -> Self {
        Self {
            state,
            default_top_k,
            sinks,
        }
    }
}

/// Classify a meta-tool call failure for the observation record.
fn classify(e: &metatools::MetaError) -> (observe::CallOutcome, Option<&'static str>) {
    use metatools::MetaError as E;
    use observe::CallOutcome as O;
    match e {
        E::Timeout => (O::Timeout, Some("timeout")),
        E::Call(_) => (O::Error, Some("upstream_call")),
        E::ToolNotFound(_) => (O::Error, Some("tool_not_found")),
        E::UpstreamUnavailable(_) => (O::Error, Some("upstream_unavailable")),
    }
}

/// A `std::io::Write` that discards bytes and only counts them, so a value's serialized JSON
/// length can be measured without allocating an intermediate `String`.
struct CountingWriter(usize);

impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialized JSON byte length of `value` without allocating a `String` (0 on serialize error).
fn json_len<T: serde::Serialize>(value: &T) -> usize {
    let mut counter = CountingWriter(0);
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => counter.0,
        Err(_) => 0,
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
        use observe::{CallOutcome, CallRecord, MetaTool};

        let args = request.arguments.unwrap_or_default();
        let arg_bytes = json_len(&args);
        // Start timing after argument-size bookkeeping so latency reflects dispatch,
        // not the observability accounting (symmetric with result_bytes below).
        let started = std::time::Instant::now();

        // Each arm yields: (response, meta_tool, target_tool, outcome, error_kind).
        // The unknown-meta-name case returns a protocol error and is NOT recorded.
        let (response, meta_tool, target_tool, outcome, error_kind): (
            Result<CallToolResult, McpError>,
            MetaTool,
            Option<String>,
            CallOutcome,
            Option<&'static str>,
        ) = match request.name.as_ref() {
            "search_tools" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let top_k = args
                    .get("top_k")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(self.default_top_k);
                let snap = self.state.snapshot();
                let hits = metatools::search_tools(&snap, query, top_k).await;
                match serde_json::to_string(&hits) {
                    Ok(json) => (
                        Ok(CallToolResult::success(vec![Content::text(json)])),
                        MetaTool::SearchTools,
                        None,
                        CallOutcome::Ok,
                        None,
                    ),
                    Err(e) => (
                        Err(McpError::internal_error(e.to_string(), None)),
                        MetaTool::SearchTools,
                        None,
                        CallOutcome::Error,
                        Some("internal"),
                    ),
                }
            }
            "get_tool_details" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let snap = self.state.snapshot();
                match metatools::get_tool_details(&snap, name) {
                    Some(def) => match serde_json::to_string(def) {
                        Ok(json) => (
                            Ok(CallToolResult::success(vec![Content::text(json)])),
                            MetaTool::GetToolDetails,
                            None,
                            CallOutcome::Ok,
                            None,
                        ),
                        Err(e) => (
                            Err(McpError::internal_error(e.to_string(), None)),
                            MetaTool::GetToolDetails,
                            None,
                            CallOutcome::Error,
                            Some("internal"),
                        ),
                    },
                    None => (
                        Ok(CallToolResult::error(vec![Content::text(format!(
                            "no such tool: {name}"
                        ))])),
                        MetaTool::GetToolDetails,
                        None,
                        CallOutcome::Error,
                        Some("tool_not_found"),
                    ),
                }
            }
            "call_tool" => match args.get("name").and_then(|v| v.as_str()) {
                None => (
                    Ok(CallToolResult::error(vec![Content::text(
                        "missing required 'name'",
                    )])),
                    MetaTool::CallTool,
                    None,
                    CallOutcome::Error,
                    Some("invalid_params"),
                ),
                Some(name) => {
                    let inner = args.get("arguments").and_then(|v| v.as_object()).cloned();
                    let snap = self.state.snapshot();
                    match metatools::call_tool(&snap, self.state.registry(), name, inner).await {
                        Ok(result) => (
                            Ok(result),
                            MetaTool::CallTool,
                            Some(name.to_string()),
                            CallOutcome::Ok,
                            None,
                        ),
                        Err(e) => {
                            let (outcome, kind) = classify(&e);
                            (
                                Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                                MetaTool::CallTool,
                                Some(name.to_string()),
                                outcome,
                                kind,
                            )
                        }
                    }
                }
            },
            other => {
                // Unknown meta-tool name: protocol error, not a gateway tool call -> not recorded.
                return Err(McpError::invalid_params(
                    format!("unknown tool: {other}"),
                    None,
                ));
            }
        };

        // Measure dispatch latency before observability bookkeeping (re-serialization,
        // upstream derivation) so the recorded value reflects the call, not the recording.
        let latency_ms = started.elapsed().as_millis() as u64;
        let result_bytes = match &response {
            Ok(r) => json_len(r),
            Err(_) => 0,
        };
        let upstream = target_tool
            .as_deref()
            .and_then(|t| t.split_once("__").map(|(s, _)| s.to_string()));
        let rec = CallRecord {
            ts_unix_ms: CallRecord::now_unix_ms(),
            meta_tool,
            target_tool,
            upstream,
            latency_ms,
            outcome,
            error_kind,
            arg_bytes,
            result_bytes,
        };
        for sink in self.sinks.iter() {
            sink.record(&rec);
        }
        response
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

    #[test]
    fn json_len_matches_to_string_len() {
        let samples = [
            serde_json::json!({}),
            serde_json::json!({"query": "weather", "top_k": 5}),
            serde_json::json!([1, 2, 3, {"nested": ["a", "b"]}, "unicode: café 日本語"]),
            serde_json::json!("plain string"),
        ];
        for v in samples {
            let expected = serde_json::to_string(&v).unwrap().len();
            assert_eq!(super::json_len(&v), expected, "json_len mismatch for {v}");
        }
    }
}
