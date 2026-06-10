//! The three meta-tool functions over an immutable `GatewaySnapshot`.

use catalog::ToolDef;

use crate::snapshot::{GatewaySnapshot, ToolSummary};

/// Search the snapshot's tools for `query`, returning up to `top_k` summaries (best first).
pub fn search_tools(snap: &GatewaySnapshot, query: &str, top_k: usize) -> Vec<ToolSummary> {
    snap.strategy
        .search(query, top_k)
        .into_iter()
        .map(|hit| ToolSummary {
            name: hit.qualified_name,
            description: hit.description,
        })
        .collect()
}

/// Look up the full definition of one tool by its namespaced (`{server}__{name}`) name.
pub fn get_tool_details<'a>(snap: &'a GatewaySnapshot, name: &str) -> Option<&'a ToolDef> {
    snap.catalog.get(name)
}

use crate::error::MetaError;
use upstream::registry::UpstreamRegistry;

/// Route a tool call: look the namespaced `name` up in the catalog to get its `(server, tool)`
/// — NEVER by splitting on `__` — then forward to that upstream via the registry.
pub async fn call_tool(
    snap: &GatewaySnapshot,
    registry: &UpstreamRegistry,
    name: &str,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<rmcp::model::CallToolResult, MetaError> {
    let def = snap
        .catalog
        .get(name)
        .ok_or_else(|| MetaError::ToolNotFound(name.to_string()))?;
    let handle = registry
        .get(&def.server)
        .ok_or_else(|| MetaError::UpstreamUnavailable(def.server.clone()))?;
    handle
        .call_tool(&def.name, arguments)
        .await
        .map_err(|e| match e {
            upstream::connection::UpstreamError::Timeout { .. } => MetaError::Timeout,
            other => MetaError::Call(other.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::Catalog;
    use retrieval::Bm25Strategy;
    use retrieval::RetrievalStrategy;
    use serde_json::Value;

    fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
        ToolDef {
            server: server.into(),
            name: name.into(),
            description: desc.into(),
            input_schema: Value::Null,
        }
    }

    fn snapshot() -> GatewaySnapshot {
        let catalog = Catalog::from_tooldefs(vec![
            tool("github", "create_issue", "Create a new issue in a GitHub repository"),
            tool("weather", "get_forecast", "Get the weather forecast for a location"),
        ]);
        let mut strat = Bm25Strategy::new();
        strat.index(&catalog);
        GatewaySnapshot::new(catalog, Box::new(strat))
    }

    #[test]
    fn search_tools_returns_namespaced_summaries() {
        let snap = snapshot();
        let hits = search_tools(&snap, "weather forecast", 5);
        assert_eq!(hits.first().map(|s| s.name.as_str()), Some("weather__get_forecast"));
        assert!(hits[0].description.contains("forecast"));
    }

    #[test]
    fn get_tool_details_returns_full_def_or_none() {
        let snap = snapshot();
        let d = get_tool_details(&snap, "github__create_issue").unwrap();
        assert_eq!(d.server, "github");
        assert_eq!(d.name, "create_issue");
        assert!(get_tool_details(&snap, "nope__missing").is_none());
    }

    #[test]
    fn get_tool_details_handles_tool_names_containing_double_underscore() {
        // A tool whose ORIGINAL name contains "__" must still be retrievable by its
        // qualified name; routing later relies on the stored `server`/`name` fields,
        // not on splitting the qualified string.
        let catalog = Catalog::from_tooldefs(vec![tool("srv", "weird__tool", "x")]);
        let mut strat = Bm25Strategy::new();
        strat.index(&catalog);
        let snap = GatewaySnapshot::new(catalog, Box::new(strat));

        let d = get_tool_details(&snap, "srv__weird__tool").unwrap();
        assert_eq!(d.server, "srv");
        assert_eq!(d.name, "weird__tool"); // a naive split on "__" would get this wrong
    }
}
