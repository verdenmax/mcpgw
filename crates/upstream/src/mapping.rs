//! Map rmcp tools into the namespaced `catalog::ToolDef`, and ingest a server's
//! tools into a catalog with duplicate detection.

use catalog::{Catalog, ToolDef};
use rmcp::model::Tool;

/// Maximum number of tools accepted from a single upstream server per ingest. Extras are
/// dropped (with a warn) to bound catalog/snapshot memory against a compromised upstream.
pub const MAX_TOOLS_PER_SERVER: usize = 1024;

/// Maximum bytes of a single tool's `name` + `description` + serialized `input_schema`. A tool
/// over this is skipped (with a warn) so one upstream can't drive unbounded memory/embedding cost.
/// The `name` is included because it is persisted twice in the snapshot (as `ToolDef.name` and as
/// the `{server}__{name}` catalog key), so the bound would otherwise be incomplete.
pub const MAX_TOOL_TEXT_BYTES: usize = 64 * 1024;

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
///
/// Dedup is per-call (intra-server, first-wins). Re-ingesting a server already present
/// in the catalog overwrites its entries via `upsert`; the returned count reflects only
/// collisions within `tools`, not against prior catalog state.
///
/// Tools beyond MAX_TOOLS_PER_SERVER, or whose name+description+schema exceeds
/// MAX_TOOL_TEXT_BYTES, are dropped with a warn (not counted in the returned dupe count).
pub fn ingest_tools(catalog: &mut Catalog, server: &str, tools: &[Tool]) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut dupes = 0;
    let mut accepted = 0usize;
    for (i, tool) in tools.iter().enumerate() {
        if accepted >= MAX_TOOLS_PER_SERVER {
            tracing::warn!(
                server,
                dropped = tools.len() - i,
                max = MAX_TOOLS_PER_SERVER,
                "upstream exceeds per-server tool cap; dropping extras"
            );
            break;
        }
        if !seen.insert(tool.name.as_ref()) {
            dupes += 1;
            tracing::warn!(server, tool = %tool.name, "duplicate tool name from upstream; keeping first");
            continue;
        }
        let text_bytes = tool.name.len()
            + tool.description.as_deref().unwrap_or("").len()
            + serde_json::to_string(&*tool.input_schema)
                .map(|s| s.len())
                .unwrap_or(0);
        if text_bytes > MAX_TOOL_TEXT_BYTES {
            tracing::warn!(
                server,
                tool = %tool.name,
                bytes = text_bytes,
                max = MAX_TOOL_TEXT_BYTES,
                "tool text exceeds size cap; skipping"
            );
            continue;
        }
        catalog.upsert(tool_to_def(server, tool));
        accepted += 1;
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
    fn tool_to_def_preserves_input_schema() {
        use rmcp::model::JsonObject;
        let mut schema = JsonObject::new();
        schema.insert("type".into(), serde_json::Value::String("object".into()));
        schema.insert(
            "required".into(),
            serde_json::Value::Array(vec![serde_json::Value::String("x".into())]),
        );
        let t = Tool::new("t".to_string(), "d".to_string(), schema);
        let d = tool_to_def("s", &t);
        assert_eq!(
            d.input_schema,
            serde_json::json!({ "type": "object", "required": ["x"] })
        );
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

    #[test]
    fn ingest_tools_caps_per_server_tool_count() {
        let mut cat = Catalog::new();
        let tools: Vec<_> = (0..(MAX_TOOLS_PER_SERVER + 5))
            .map(|i| tool(&format!("t{i}"), Some("d")))
            .collect();
        let dupes = ingest_tools(&mut cat, "srv", &tools);
        assert_eq!(dupes, 0, "all names unique -> no intra-server dupes");
        assert_eq!(
            cat.len(),
            MAX_TOOLS_PER_SERVER,
            "extras beyond the per-server cap must be dropped"
        );
    }

    #[test]
    fn ingest_tools_skips_a_tool_over_the_text_byte_cap() {
        let mut cat = Catalog::new();
        // empty schema serializes to "{}" (2 bytes); description pushes total over the cap.
        let huge = "a".repeat(MAX_TOOL_TEXT_BYTES + 1);
        let tools = vec![
            tool("small", Some("ok")),
            tool("huge", Some(&huge)),
            tool("also_small", Some("ok2")),
        ];
        let dupes = ingest_tools(&mut cat, "srv", &tools);
        assert_eq!(dupes, 0);
        assert_eq!(cat.len(), 2, "the oversize tool is skipped, others kept");
        assert!(cat.get("srv__small").is_some());
        assert!(cat.get("srv__also_small").is_some());
        assert!(cat.get("srv__huge").is_none(), "oversize tool excluded");
    }

    #[test]
    fn ingest_tools_accepts_a_tool_exactly_at_the_text_byte_cap() {
        let mut cat = Catalog::new();
        // text_bytes = name + description + serialized schema. Empty schema is "{}" (2 bytes);
        // with name "edge" (4 bytes), a description of MAX-6 lands exactly at MAX, which is NOT
        // over the cap (strict `>`), so it is accepted.
        let name = "edge";
        let at_limit = "a".repeat(MAX_TOOL_TEXT_BYTES - 2 - name.len());
        let tools = vec![tool(name, Some(&at_limit))];
        ingest_tools(&mut cat, "srv", &tools);
        assert!(
            cat.get("srv__edge").is_some(),
            "a tool exactly at the byte cap must be accepted"
        );
    }

    #[test]
    fn ingest_tools_skips_a_tool_with_an_oversize_name() {
        let mut cat = Catalog::new();
        // The byte cap includes the name (persisted as ToolDef.name and the catalog key), so a
        // tiny-description tool with a huge name is still skipped.
        let huge_name = "n".repeat(MAX_TOOL_TEXT_BYTES + 1);
        let tools = vec![tool("small", Some("ok")), tool(&huge_name, Some("d"))];
        let dupes = ingest_tools(&mut cat, "srv", &tools);
        assert_eq!(dupes, 0);
        assert_eq!(
            cat.len(),
            1,
            "the oversize-name tool is skipped, the small one kept"
        );
        assert!(cat.get("srv__small").is_some());
    }
}
