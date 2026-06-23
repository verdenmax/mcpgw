// Enumerations + section metadata shared by the form sections and the validator.
export const STRATEGIES = ["bm25", "vector", "hybrid", "subagent"];
export const TRANSPORTS = ["stdio", "http"];

// Left-nav order of top-level sections (TOML-native key names).
export const SECTIONS = ["retrieval", "server", "audit", "dashboard", "upstream"];

// Only `[[upstream]]` hot-reloads; everything else needs a restart.
export const HOT_RELOAD_SECTIONS = ["upstream"];
export function sectionReload(section) {
  return HOT_RELOAD_SECTIONS.includes(section) ? "hot" : "restart";
}

// Default value for a section when the user enables it from the form (aligned with the
// config crate's #[serde(default)] sensible defaults). `upstream` is handled as an array.
export function defaultSection(name) {
  switch (name) {
    case "retrieval": return { strategy: "bm25", top_k: 8 };
    case "server": return { stdio: true };
    case "audit": return { enabled: false, path: "mcpgw-audit.jsonl" };
    case "dashboard":
      return { enabled: false, bind: "127.0.0.1:8971", trace_queries: false,
               trace_buffer: 500, call_buffer: 2000, payload_max_bytes: 16384 };
    default: return {};
  }
}
