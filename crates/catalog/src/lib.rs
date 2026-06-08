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
}
