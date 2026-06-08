//! Map rmcp tools into the namespaced `catalog::ToolDef`, and ingest a server's
//! tools into a catalog with duplicate detection.

use catalog::{Catalog, ToolDef};
use rmcp::model::Tool;

/// Convert one upstream `Tool` (under namespace `server`) into a `ToolDef`.
pub fn tool_to_def(server: &str, tool: &Tool) -> ToolDef {
    ToolDef {
        server: server.to_string(),
        name: tool.name.to_string(),
        description: tool.description.as_deref().unwrap_or("").to_string(),
        input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
    }
}

/// Ingest a server's tools into `catalog`. Returns the number of intra-server
/// duplicate tool names that were skipped (already warned via tracing).
pub fn ingest_tools(catalog: &mut Catalog, server: &str, tools: &[Tool]) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut dupes = 0;
    for tool in tools {
        if !seen.insert(tool.name.to_string()) {
            dupes += 1;
            tracing::warn!(server, tool = %tool.name, "duplicate tool name from upstream; keeping first");
            continue;
        }
        catalog.upsert(tool_to_def(server, tool));
    }
    dupes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, desc: Option<&str>) -> Tool {
        use rmcp::model::JsonObject;
        let mut t = Tool::new(
            name.to_string(),
            desc.unwrap_or("").to_string(),
            JsonObject::new(),
        );
        if desc.is_none() {
            t.description = None;
        }
        t
    }

    #[test]
    fn tool_to_def_namespaces_and_copies_fields() {
        let d = tool_to_def("github", &tool("create_issue", Some("Create an issue")));
        assert_eq!(d.qualified_name(), "github__create_issue");
        assert_eq!(d.server, "github");
        assert_eq!(d.name, "create_issue");
        assert_eq!(d.description, "Create an issue");
    }

    #[test]
    fn tool_to_def_handles_missing_description() {
        let d = tool_to_def("s", &tool("t", None));
        assert_eq!(d.description, "");
    }

    #[test]
    fn ingest_tools_adds_namespaced_and_counts_dupes() {
        let mut cat = Catalog::new();
        let tools = vec![
            tool("a", Some("first a")),
            tool("b", Some("b")),
            tool("a", Some("second a")),
        ];
        let dupes = ingest_tools(&mut cat, "srv", &tools);
        assert_eq!(dupes, 1);
        assert_eq!(cat.len(), 2);
        assert_eq!(cat.get("srv__a").unwrap().description, "first a"); // first kept
        assert!(cat.get("srv__b").is_some());
    }
}
