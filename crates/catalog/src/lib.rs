use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single tool exposed by an upstream MCP server, as stored in the catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    /// Upstream server namespace (e.g. "github").
    pub server: String,
    /// Original tool name within that server (e.g. "create_issue").
    pub name: String,
    /// One-line human description used for retrieval and `search_tools` output.
    pub description: String,
    /// Full JSON input schema, returned by `get_tool_details`.
    #[serde(default)]
    pub input_schema: Value,
}

impl ToolDef {
    /// Namespaced, collision-free identifier: `{server}__{name}`.
    pub fn qualified_name(&self) -> String {
        format!("{}__{}", self.server, self.name)
    }
}

use std::collections::BTreeMap;

/// In-memory registry of all tools across upstream servers, keyed by qualified name.
#[derive(Debug, Default, Clone)]
pub struct Catalog {
    tools: BTreeMap<String, ToolDef>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a tool (keyed by its qualified name).
    pub fn upsert(&mut self, tool: ToolDef) {
        self.tools.insert(tool.qualified_name(), tool);
    }

    /// Remove every tool belonging to `server`.
    pub fn remove_server(&mut self, server: &str) {
        self.tools.retain(|_, t| t.server != server);
    }

    /// Look up a tool by qualified name (e.g. "github__create_issue").
    pub fn get(&self, qualified_name: &str) -> Option<&ToolDef> {
        self.tools.get(qualified_name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Iterate over all tools in deterministic (qualified-name) order.
    pub fn iter(&self) -> impl Iterator<Item = &ToolDef> {
        self.tools.values()
    }

    /// Build a catalog from a flat list of tools.
    pub fn from_tooldefs(tools: Vec<ToolDef>) -> Self {
        let mut c = Catalog::new();
        for t in tools {
            c.upsert(t);
        }
        c
    }
}

/// Error returned when loading a catalog from JSON.
#[derive(Debug)]
pub struct CatalogLoadError(pub serde_json::Error);

impl std::fmt::Display for CatalogLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to parse catalog JSON: {}", self.0)
    }
}

impl std::error::Error for CatalogLoadError {}

impl Catalog {
    /// Parse a JSON array of `ToolDef` objects into a `Catalog`.
    pub fn from_json_str(json: &str) -> Result<Self, CatalogLoadError> {
        let tools: Vec<ToolDef> = serde_json::from_str(json).map_err(CatalogLoadError)?;
        Ok(Catalog::from_tooldefs(tools))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_joins_server_and_name_with_double_underscore() {
        let t = ToolDef {
            server: "github".into(),
            name: "create_issue".into(),
            description: "Create a GitHub issue".into(),
            input_schema: Value::Null,
        };
        assert_eq!(t.qualified_name(), "github__create_issue");
    }

    fn tool(server: &str, name: &str) -> ToolDef {
        ToolDef {
            server: server.into(),
            name: name.into(),
            description: format!("{server} {name}"),
            input_schema: Value::Null,
        }
    }

    #[test]
    fn catalog_upsert_get_and_remove_server() {
        let mut c = Catalog::new();
        c.upsert(tool("github", "create_issue"));
        c.upsert(tool("github", "list_repos"));
        c.upsert(tool("slack", "post_message"));

        assert_eq!(c.len(), 3);
        assert_eq!(
            c.get("github__create_issue").map(|t| t.name.as_str()),
            Some("create_issue")
        );

        // upsert with the same qualified name replaces, not duplicates.
        let mut updated = tool("github", "create_issue");
        updated.description = "updated".into();
        c.upsert(updated);
        assert_eq!(c.len(), 3);
        assert_eq!(c.get("github__create_issue").unwrap().description, "updated");

        // removing a server drops only its tools.
        c.remove_server("github");
        assert_eq!(c.len(), 1);
        assert!(c.get("slack__post_message").is_some());
    }

    #[test]
    fn from_json_str_parses_array_of_tools() {
        let json = r#"
        [
          {"server":"github","name":"create_issue","description":"Create an issue",
           "input_schema":{"type":"object"}},
          {"server":"slack","name":"post_message","description":"Post a message"}
        ]"#;
        let c = Catalog::from_json_str(json).expect("valid json");
        assert_eq!(c.len(), 2);
        assert_eq!(
            c.get("github__create_issue").unwrap().description,
            "Create an issue"
        );
        // input_schema defaults to Null when omitted.
        assert_eq!(c.get("slack__post_message").unwrap().input_schema, Value::Null);
    }

    #[test]
    fn from_json_str_rejects_invalid_json() {
        assert!(Catalog::from_json_str("not json").is_err());
    }
}
