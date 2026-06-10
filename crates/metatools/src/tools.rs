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
}
