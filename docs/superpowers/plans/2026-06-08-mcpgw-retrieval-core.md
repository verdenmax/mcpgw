# mcpgw Retrieval Core (Plan 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the dependency-light retrieval core of mcpgw — a Rust workspace that loads a namespaced tool catalog and answers `search`/`get-details` queries via a pluggable BM25 strategy, exposed through a CLI.

**Architecture:** A Cargo workspace with four focused crates: `catalog` (namespaced tool registry + JSON load), `retrieval` (a `RetrievalStrategy` trait + in-house BM25 implementation + strategy factory), `config` (TOML config for the `[retrieval]` section), and a root `mcpgw` binary (clap CLI that wires them together). No MCP SDK or network I/O in this plan — that is Plan 2. This isolates and de-risks the project's core differentiator (intelligent tool retrieval) and makes it fully unit/golden testable.

**Tech Stack:** Rust (edition 2021), `serde` + `serde_json` + `toml` for data, `thiserror` for errors, `clap` for the CLI. BM25 is implemented in-house (pure Rust) over the small in-memory catalog; `tantivy` remains the documented upgrade path behind the same trait.

> **Scope / spec note:** This is Plan 1 of the P1 milestone in
> `docs/superpowers/specs/2026-06-08-mcpgw-progressive-discovery-design.md`.
> It implements the **Tool Catalog (④)**, **Retrieval Engine (⑤, BM25 default)**, and the
> **`[retrieval]` part of Config (⑥)**. The live MCP I/O layer — Upstream Manager (③),
> Downstream Server (①), Meta-Tool Layer (②), Router (⑦) using `rmcp` — is **Plan 2**.
> Per the spec, the default strategy is `hybrid` (BM25+vector); v1 ships **BM25 first** and
> returns a clear "not implemented in v1" error for `vector`/`hybrid` so Plan 2/P2 can slot them in.

---

## File Structure

```
mcpgw/
├─ Cargo.toml                      # virtual workspace (members, resolver) — NOT a package
├─ crates/
│  ├─ catalog/
│  │  ├─ Cargo.toml
│  │  └─ src/lib.rs                # ToolDef, Catalog (namespacing, upsert/remove/get, JSON load)
│  ├─ retrieval/
│  │  ├─ Cargo.toml
│  │  ├─ src/lib.rs                # ScoredTool, RetrievalStrategy trait, tokenize, Bm25Strategy, build_strategy
│  │  └─ tests/golden.rs           # golden top-1 ranking test
│  ├─ config/
│  │  ├─ Cargo.toml
│  │  └─ src/lib.rs                # Config, RetrievalConfig, load(), validation
│  └─ mcpgw/
│     ├─ Cargo.toml                # binary crate, package name = "mcpgw"
│     ├─ src/main.rs               # clap CLI (search / get-details)
│     └─ tests/cli.rs              # integration test invoking the built `mcpgw` binary
├─ tests/
│  └─ fixtures/tools.json          # shared sample catalog (workspace root)
└─ docs/...                        # (already present)
```

> **Why a virtual workspace?** The root manifest declares only `[workspace]` (no `[package]`),
> so the binary lives in its own member crate `crates/mcpgw`. This avoids the trap where the
> workspace-root package would be "targetless" before `src/main.rs` exists, and keeps each
> crate's responsibility clean.

Responsibilities:
- **`catalog`**: owns the tool data model and namespacing. No knowledge of search or config.
- **`retrieval`**: owns ranking. Depends on `catalog` types only. No knowledge of config files or CLI.
- **`config`**: owns parsing/validating the `[retrieval]` TOML section. No knowledge of search internals.
- **`mcpgw` (bin crate)**: wires catalog + config + retrieval into a CLI. The only crate that knows about all three.

---

## Task 1: Workspace scaffold + `catalog` crate with `ToolDef`

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/catalog/Cargo.toml`
- Create: `crates/catalog/src/lib.rs`

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

Create `Cargo.toml` (a **virtual** manifest — no `[package]`, so the workspace root is not itself a crate):

```toml
[workspace]
resolver = "2"
# Start with only the crate that exists. A virtual workspace fails to LOAD if any
# listed member's Cargo.toml is missing, so later tasks add their crate here as they
# create it (Task 4 -> retrieval, Task 7 -> config, Task 9 -> mcpgw).
members = ["crates/catalog"]

[workspace.package]
edition = "2021"
rust-version = "1.86"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
thiserror = "1"
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 2: Create the `catalog` crate manifest**

Create `crates/catalog/Cargo.toml`:

```toml
[package]
name = "catalog"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 3: Write the failing test for `ToolDef::qualified_name`**

Create `crates/catalog/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Run the test to verify it passes (and the workspace builds)**

Run: `cargo test -p catalog`
Expected: compiles; `1 passed`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/catalog
git commit -m "feat(catalog): workspace scaffold + ToolDef with namespaced qualified_name"
```

---

## Task 2: `Catalog` registry operations

**Files:**
- Modify: `crates/catalog/src/lib.rs`

- [ ] **Step 1: Write the failing test for `Catalog` upsert/get/remove_server**

Add to the `tests` module in `crates/catalog/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p catalog catalog_upsert_get_and_remove_server`
Expected: FAIL — `cannot find type Catalog in this scope`.

- [ ] **Step 3: Implement `Catalog`**

Add to `crates/catalog/src/lib.rs` (after the `ToolDef` impl, before `#[cfg(test)]`):

```rust
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p catalog`
Expected: `2 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/catalog/src/lib.rs
git commit -m "feat(catalog): Catalog registry with upsert/get/remove_server/iter"
```

---

## Task 3: Load a catalog from JSON

**Files:**
- Modify: `crates/catalog/src/lib.rs`

- [ ] **Step 1: Write the failing test for `Catalog::from_json_str`**

Add to the `tests` module:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p catalog from_json_str`
Expected: FAIL — `no function or associated item named from_json_str`.

- [ ] **Step 3: Implement JSON loading**

Add an error type and method to `crates/catalog/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p catalog`
Expected: `4 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/catalog/src/lib.rs
git commit -m "feat(catalog): load catalog from a JSON array of tools"
```

---

## Task 4: `retrieval` crate — trait, `ScoredTool`, tokenizer

**Files:**
- Create: `crates/retrieval/Cargo.toml`
- Create: `crates/retrieval/src/lib.rs`

- [ ] **Step 1: Create the `retrieval` crate manifest**

Create `crates/retrieval/Cargo.toml`:

```toml
[package]
name = "retrieval"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
catalog = { path = "../catalog" }
thiserror = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

> The `serde_json` dev-dependency is only needed by tests that construct `ToolDef`
> values directly (its `input_schema` field is a `serde_json::Value`).

- [ ] **Step 1b: Register the crate in the workspace**

Edit the root `Cargo.toml` `members` to add the new crate (a virtual workspace fails to load
if a member manifest is missing, so it is added only now that it exists):

```toml
members = ["crates/catalog", "crates/retrieval"]
```

- [ ] **Step 2: Write the failing test for `tokenize`**

Create `crates/retrieval/src/lib.rs`:

```rust
use catalog::Catalog;

/// A retrieval hit: a tool's qualified name, its description, and a relevance score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredTool {
    pub qualified_name: String,
    pub description: String,
    pub score: f32,
}

/// A pluggable tool-retrieval strategy (BM25, vector, hybrid, ...).
pub trait RetrievalStrategy: Send + Sync {
    /// (Re)build internal indices from the current catalog.
    fn index(&mut self, catalog: &Catalog);
    /// Return up to `top_k` tools relevant to `query`, best first.
    fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}

/// Lowercase, split on any non-alphanumeric boundary (this also splits `_`), drop empties.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_on_non_alphanumeric_and_lowercases() {
        assert_eq!(
            tokenize("GitHub__create_issue"),
            vec!["github", "create", "issue"]
        );
        assert_eq!(tokenize("  multiple,, spaces "), vec!["multiple", "spaces"]);
        assert!(tokenize("").is_empty());
    }
}
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p retrieval`
Expected: compiles; `1 passed`.

- [ ] **Step 4: Commit**

```bash
git add crates/retrieval
git commit -m "feat(retrieval): RetrievalStrategy trait, ScoredTool, tokenizer"
```

---

## Task 5: `Bm25Strategy` — index + search

**Files:**
- Modify: `crates/retrieval/src/lib.rs`

- [ ] **Step 1: Write the failing test for BM25 ranking**

Add to the `tests` module in `crates/retrieval/src/lib.rs`:

```rust
    use catalog::ToolDef;
    use serde_json::Value;

    fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
        ToolDef {
            server: server.into(),
            name: name.into(),
            description: desc.into(),
            input_schema: Value::Null,
        }
    }

    fn sample_catalog() -> Catalog {
        Catalog::from_tooldefs(vec![
            tool("github", "create_issue", "Create a new issue in a GitHub repository"),
            tool("github", "list_pull_requests", "List pull requests for a repository"),
            tool("slack", "post_message", "Send a chat message to a Slack channel"),
            tool("weather", "get_forecast", "Get the weather forecast for a location"),
        ])
    }

    #[test]
    fn bm25_ranks_relevant_tool_first() {
        let mut s = Bm25Strategy::new();
        s.index(&sample_catalog());

        let hits = s.search("create github issue", 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].qualified_name, "github__create_issue");
        // scores are sorted descending
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[test]
    fn bm25_respects_top_k_and_filters_zero_score() {
        let mut s = Bm25Strategy::new();
        s.index(&sample_catalog());

        // Only weather matches; top_k larger than match count returns just the match.
        let hits = s.search("forecast", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].qualified_name, "weather__get_forecast");

        // No term matches -> empty.
        assert!(s.search("zzzzz nonexistent", 10).is_empty());

        // top_k caps the result count.
        let capped = s.search("repository", 1);
        assert_eq!(capped.len(), 1);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrieval bm25`
Expected: FAIL — `cannot find type Bm25Strategy in this scope`.

- [ ] **Step 3: Implement `Bm25Strategy`**

Add to `crates/retrieval/src/lib.rs` (after `tokenize`, before `#[cfg(test)]`):

```rust
use std::collections::HashMap;

/// A single indexed document (one per tool). The searchable text is
/// the tool's qualified name plus its description.
#[derive(Debug, Clone)]
struct IndexedDoc {
    qualified_name: String,
    description: String,
    len: usize,
    term_freq: HashMap<String, u32>,
}

/// In-house BM25 ranking over the (small) tool catalog.
///
/// Deterministic and dependency-free, ideal for golden tests. For very large
/// catalogs, swap in a `tantivy`-backed strategy behind the same trait.
#[derive(Debug, Clone)]
pub struct Bm25Strategy {
    k1: f32,
    b: f32,
    docs: Vec<IndexedDoc>,
    doc_freq: HashMap<String, u32>,
    avgdl: f32,
    n: usize,
}

impl Bm25Strategy {
    pub fn new() -> Self {
        Self {
            k1: 1.2,
            b: 0.75,
            docs: Vec::new(),
            doc_freq: HashMap::new(),
            avgdl: 0.0,
            n: 0,
        }
    }

    fn idf(&self, term: &str) -> f32 {
        let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
        // BM25 idf with +1 to keep it non-negative.
        (((self.n as f32 - df + 0.5) / (df + 0.5)) + 1.0).ln()
    }
}

impl Default for Bm25Strategy {
    fn default() -> Self {
        Self::new()
    }
}

impl RetrievalStrategy for Bm25Strategy {
    fn index(&mut self, catalog: &Catalog) {
        let mut docs = Vec::new();
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        let mut total_len = 0usize;

        for tool in catalog.iter() {
            let mut text = tool.qualified_name();
            text.push(' ');
            text.push_str(&tool.description);
            let tokens = tokenize(&text);

            let mut term_freq: HashMap<String, u32> = HashMap::new();
            for tok in &tokens {
                *term_freq.entry(tok.clone()).or_insert(0) += 1;
            }
            for term in term_freq.keys() {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }

            total_len += tokens.len();
            docs.push(IndexedDoc {
                qualified_name: tool.qualified_name(),
                description: tool.description.clone(),
                len: tokens.len(),
                term_freq,
            });
        }

        self.n = docs.len();
        self.avgdl = if self.n == 0 {
            0.0
        } else {
            total_len as f32 / self.n as f32
        };
        self.doc_freq = doc_freq;
        self.docs = docs;
    }

    fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        if self.n == 0 || self.avgdl == 0.0 {
            return Vec::new();
        }
        let q_terms = tokenize(query);

        let mut scored: Vec<ScoredTool> = self
            .docs
            .iter()
            .map(|doc| {
                let mut score = 0.0f32;
                for term in &q_terms {
                    if let Some(&f) = doc.term_freq.get(term) {
                        let f = f as f32;
                        let denom = f
                            + self.k1
                                * (1.0 - self.b + self.b * (doc.len as f32 / self.avgdl));
                        score += self.idf(term) * (f * (self.k1 + 1.0)) / denom;
                    }
                }
                ScoredTool {
                    qualified_name: doc.qualified_name.clone(),
                    description: doc.description.clone(),
                    score,
                }
            })
            .filter(|s| s.score > 0.0)
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.qualified_name.cmp(&b.qualified_name))
        });
        scored.truncate(top_k);
        scored
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p retrieval`
Expected: `3 passed` (tokenize + two bm25 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/retrieval/src/lib.rs
git commit -m "feat(retrieval): in-house BM25 strategy (index + ranked search)"
```

---

## Task 6: Golden retrieval test over a fixture catalog

**Files:**
- Create: `tests/fixtures/tools.json`
- Create: `crates/retrieval/tests/golden.rs`

- [ ] **Step 1: Create the shared fixture catalog**

Create `tests/fixtures/tools.json`:

```json
[
  {"server":"github","name":"create_issue","description":"Create a new issue in a GitHub repository"},
  {"server":"github","name":"list_pull_requests","description":"List open pull requests for a repository"},
  {"server":"github","name":"merge_pull_request","description":"Merge a pull request into the base branch"},
  {"server":"slack","name":"post_message","description":"Send a chat message to a Slack channel"},
  {"server":"slack","name":"list_channels","description":"List available Slack channels"},
  {"server":"weather","name":"get_forecast","description":"Get the weather forecast for a location"},
  {"server":"filesystem","name":"read_file","description":"Read the contents of a file from disk"},
  {"server":"filesystem","name":"write_file","description":"Write contents to a file on disk"}
]
```

- [ ] **Step 2: Write the failing golden test**

Create `crates/retrieval/tests/golden.rs`:

```rust
use catalog::Catalog;
use retrieval::{Bm25Strategy, RetrievalStrategy};

fn load_catalog() -> Catalog {
    // `cargo test` sets CWD to the crate's manifest dir (crates/retrieval) for a
    // workspace member, so resolve the workspace-root fixture via CARGO_MANIFEST_DIR.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/tools.json");
    let json =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    Catalog::from_json_str(&json).expect("fixture must be valid")
}

/// (query, expected top-1 qualified_name)
const GOLDEN: &[(&str, &str)] = &[
    ("merge a pull request", "github__merge_pull_request"),
    ("send slack chat message", "slack__post_message"),
    ("weather forecast", "weather__get_forecast"),
    ("write file to disk", "filesystem__write_file"),
];

#[test]
fn golden_top_one_matches_expected() {
    let mut s = Bm25Strategy::new();
    s.index(&load_catalog());

    for (query, expected) in GOLDEN {
        let hits = s.search(query, 5);
        assert!(!hits.is_empty(), "query {query:?} returned no hits");
        assert_eq!(
            &hits[0].qualified_name, expected,
            "query {query:?} -> got {:?}, want {expected:?}",
            hits[0].qualified_name
        );
    }
}
```

- [ ] **Step 3: Run the golden test to verify it passes**

Run: `cargo test -p retrieval --test golden`
Expected: PASS. (The fixture is located via `CARGO_MANIFEST_DIR`, so it resolves regardless of CWD.)

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/tools.json crates/retrieval/tests/golden.rs
git commit -m "test(retrieval): golden top-1 ranking over fixture catalog"
```

---

## Task 7: `config` crate — `[retrieval]` section

**Files:**
- Create: `crates/config/Cargo.toml`
- Create: `crates/config/src/lib.rs`

- [ ] **Step 1: Create the `config` crate manifest**

Create `crates/config/Cargo.toml`:

```toml
[package]
name = "config"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
serde = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 1b: Register the crate in the workspace**

Edit the root `Cargo.toml` `members` to add the new crate:

```toml
members = ["crates/catalog", "crates/retrieval", "crates/config"]
```

- [ ] **Step 2: Write the failing tests for config parsing + validation**

Create `crates/config/src/lib.rs`:

```rust
use serde::Deserialize;
use thiserror::Error;

/// Top-level mcpgw configuration. Only the `[retrieval]` section exists in Plan 1;
/// `[server]` and `[[upstream]]` are added in Plan 2.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub retrieval: RetrievalConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    /// "bm25" | "vector" | "hybrid". Only "bm25" is implemented in v1.
    pub strategy: String,
    /// Number of tools `search_tools` returns.
    pub top_k: usize,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            strategy: "bm25".into(),
            top_k: 8,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl Config {
    /// Parse and validate config from a TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        const KNOWN: [&str; 3] = ["bm25", "vector", "hybrid"];
        if !KNOWN.contains(&self.retrieval.strategy.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "unknown retrieval.strategy {:?} (expected one of {KNOWN:?})",
                self.retrieval.strategy
            )));
        }
        if self.retrieval.top_k == 0 {
            return Err(ConfigError::Invalid("retrieval.top_k must be > 0".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(cfg.retrieval.strategy, "bm25");
        assert_eq!(cfg.retrieval.top_k, 8);
    }

    #[test]
    fn parses_retrieval_section() {
        let cfg = Config::from_toml_str(
            r#"
            [retrieval]
            strategy = "hybrid"
            top_k = 5
            "#,
        )
        .unwrap();
        assert_eq!(cfg.retrieval.strategy, "hybrid");
        assert_eq!(cfg.retrieval.top_k, 5);
    }

    #[test]
    fn rejects_unknown_strategy() {
        let err = Config::from_toml_str("[retrieval]\nstrategy = \"magic\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_zero_top_k() {
        let err = Config::from_toml_str("[retrieval]\ntop_k = 0\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p config`
Expected: `4 passed`.

- [ ] **Step 4: Commit**

```bash
git add crates/config
git commit -m "feat(config): parse and validate the [retrieval] config section"
```

---

## Task 8: Strategy factory `build_strategy`

**Files:**
- Modify: `crates/retrieval/Cargo.toml`
- Modify: `crates/retrieval/src/lib.rs`

- [ ] **Step 1: Add the `config` dependency to `retrieval`**

Edit `crates/retrieval/Cargo.toml` `[dependencies]` to add:

```toml
config = { path = "../config" }
```

- [ ] **Step 2: Write the failing test for `build_strategy`**

Add to the `tests` module in `crates/retrieval/src/lib.rs`:

```rust
    use config::RetrievalConfig;

    #[test]
    fn build_strategy_returns_bm25_and_indexes() {
        let cfg = RetrievalConfig { strategy: "bm25".into(), top_k: 8 };
        let mut strat = build_strategy(&cfg).expect("bm25 is supported");
        strat.index(&sample_catalog());
        let hits = strat.search("forecast", 8);
        assert_eq!(hits.first().map(|h| h.qualified_name.as_str()), Some("weather__get_forecast"));
    }

    #[test]
    fn build_strategy_errors_on_unimplemented_strategies() {
        for s in ["vector", "hybrid"] {
            let cfg = RetrievalConfig { strategy: s.into(), top_k: 8 };
            assert!(matches!(build_strategy(&cfg), Err(StrategyError::NotImplemented(_))));
        }
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p retrieval build_strategy`
Expected: FAIL — `cannot find function build_strategy` / `cannot find type StrategyError`.

- [ ] **Step 4: Implement the factory and error type**

Add to `crates/retrieval/src/lib.rs` (after the `Bm25Strategy` impl, before `#[cfg(test)]`):

```rust
use config::RetrievalConfig;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("retrieval strategy {0:?} is not implemented in this version")]
    NotImplemented(String),
}

/// Construct a retrieval strategy from config. Only "bm25" is implemented in v1;
/// "vector" and "hybrid" are reserved for P2 and return `NotImplemented`.
pub fn build_strategy(
    cfg: &RetrievalConfig,
) -> Result<Box<dyn RetrievalStrategy>, StrategyError> {
    match cfg.strategy.as_str() {
        "bm25" => Ok(Box::new(Bm25Strategy::new())),
        other => Err(StrategyError::NotImplemented(other.to_string())),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p retrieval`
Expected: all retrieval tests pass (tokenize, 2× bm25, 2× build_strategy).

- [ ] **Step 6: Commit**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/src/lib.rs
git commit -m "feat(retrieval): build_strategy factory (bm25 impl, vector/hybrid reserved)"
```

---

## Task 9: `mcpgw` CLI binary + integration test

**Files:**
- Create: `crates/mcpgw/Cargo.toml`
- Create: `crates/mcpgw/src/main.rs`
- Create: `crates/mcpgw/tests/cli.rs`
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Create the `mcpgw` binary crate manifest**

Create `crates/mcpgw/Cargo.toml`:

```toml
[package]
name = "mcpgw"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
catalog = { path = "../catalog" }
retrieval = { path = "../retrieval" }
config = { path = "../config" }
serde_json = { workspace = true }
clap = { workspace = true }
```

- [ ] **Step 1b: Register the crate in the workspace**

Edit the root `Cargo.toml` `members` to add the final crate (and remove the now-obsolete
"future members" comment from Task 1):

```toml
members = ["crates/catalog", "crates/retrieval", "crates/config", "crates/mcpgw"]
```

- [ ] **Step 2: Implement the CLI**

Create `crates/mcpgw/src/main.rs`:

```rust
use std::path::PathBuf;
use std::process::ExitCode;

use catalog::Catalog;
use clap::{Parser, Subcommand};
use config::Config;
use retrieval::build_strategy;

/// mcpgw retrieval-core CLI: query a tool catalog with the configured strategy.
#[derive(Parser)]
#[command(name = "mcpgw", version)]
struct Cli {
    /// Path to a catalog JSON file (array of tools).
    #[arg(long, global = true, default_value = "tests/fixtures/tools.json")]
    catalog: PathBuf,
    /// Optional path to a TOML config file; defaults are used if omitted.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search for tools matching a natural-language query.
    Search {
        /// The query text.
        query: String,
        /// Override the configured top_k.
        #[arg(long)]
        top_k: Option<usize>,
    },
    /// Print the full definition of one tool by qualified name.
    GetDetails {
        /// Qualified name, e.g. "github__create_issue".
        name: String,
    },
}

fn load_config(path: &Option<PathBuf>) -> Result<Config, String> {
    match path {
        None => Ok(Config::default_from_empty()),
        Some(p) => {
            let s = std::fs::read_to_string(p).map_err(|e| format!("read config {p:?}: {e}"))?;
            Config::from_toml_str(&s).map_err(|e| e.to_string())
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let json = std::fs::read_to_string(&cli.catalog)
        .map_err(|e| format!("read catalog {:?}: {e}", cli.catalog))?;
    let catalog = Catalog::from_json_str(&json).map_err(|e| e.to_string())?;
    let cfg = load_config(&cli.config)?;

    match cli.command {
        Command::Search { query, top_k } => {
            let mut strat = build_strategy(&cfg.retrieval).map_err(|e| e.to_string())?;
            strat.index(&catalog);
            let k = top_k.unwrap_or(cfg.retrieval.top_k);
            let hits = strat.search(&query, k);
            let out: Vec<_> = hits
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "name": h.qualified_name,
                        "description": h.description,
                        "score": h.score,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        Command::GetDetails { name } => match catalog.get(&name) {
            Some(tool) => println!("{}", serde_json::to_string_pretty(tool).unwrap()),
            None => return Err(format!("no such tool: {name}")),
        },
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 3: Add the `Config::default_from_empty` helper used by the CLI**

Add to the `impl Config` block in `crates/config/src/lib.rs`:

```rust
    /// Convenience constructor returning the all-defaults config.
    pub fn default_from_empty() -> Self {
        // Parsing an empty document applies every `#[serde(default)]`.
        Config::from_toml_str("").expect("empty config is always valid")
    }
```

- [ ] **Step 4: Write the failing integration test**

Create `crates/mcpgw/tests/cli.rs`:

```rust
use std::path::PathBuf;
use std::process::Command;

/// Path to the compiled `mcpgw` binary provided by Cargo to integration tests.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpgw")
}

/// Shared fixture at the workspace root, resolved relative to this crate so it
/// works regardless of the test's current working directory.
fn fixture() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/tools.json")
}

#[test]
fn search_subcommand_returns_relevant_tool() {
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("search")
        .arg("weather forecast")
        .arg("--top-k")
        .arg("1")
        .output()
        .expect("run mcpgw");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("weather__get_forecast"), "stdout was: {stdout}");
}

#[test]
fn get_details_subcommand_prints_tool() {
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("get-details")
        .arg("github__create_issue")
        .output()
        .expect("run mcpgw");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"name\": \"create_issue\""), "stdout was: {stdout}");
}

#[test]
fn get_details_unknown_tool_fails() {
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("get-details")
        .arg("nope__missing")
        .output()
        .expect("run mcpgw");
    assert!(!out.status.success());
}
```

- [ ] **Step 5: Run the integration tests to verify they pass**

Run: `cargo test -p mcpgw --test cli`
Expected: `3 passed`. (Cargo builds the `mcpgw` binary first and exposes `CARGO_BIN_EXE_mcpgw`; the fixture is passed explicitly via `--catalog`.)

- [ ] **Step 6: Run the full workspace test suite**

Run: `cargo test`
Expected: all unit, golden, and CLI tests pass across `catalog`, `retrieval`, `config`, and the `mcpgw` binary crate.

- [ ] **Step 7: Commit**

```bash
git add crates/mcpgw crates/config/src/lib.rs
git commit -m "feat(cli): mcpgw search/get-details CLI over the retrieval core"
```

---

## Task 10: Lint, format, and a `.gitignore`

**Files:**
- Create: `.gitignore`

- [ ] **Step 1: Create `.gitignore`**

Create `.gitignore`:

```gitignore
/target
```

> This workspace ships a deployable binary (`mcpgw`), so `Cargo.lock` **is committed** (it is not
> listed here) to keep builds reproducible. The first commit that runs `cargo` will generate it;
> add it explicitly in the commit below.

- [ ] **Step 2: Format the code**

Run: `cargo fmt --all`
Expected: exits 0; files reformatted if needed.

- [ ] **Step 3: Lint with clippy (warnings as errors)**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: exits 0 with no warnings. Fix any clippy findings, then re-run until clean.

- [ ] **Step 4: Final full test run**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add .gitignore Cargo.lock
git commit -m "chore: add .gitignore, commit Cargo.lock; format and lint clean"
```

---

## Definition of Done (Plan 1)

- `cargo test` passes (unit + golden + CLI integration).
- `cargo clippy --all-targets -- -D warnings` is clean.
- `./target/debug/mcpgw search "weather forecast"` (run from the workspace root, so the default
  `--catalog tests/fixtures/tools.json` resolves) prints `weather__get_forecast` first.
- `./target/debug/mcpgw get-details github__create_issue` prints the tool's JSON.
- The `RetrievalStrategy` trait + `build_strategy` factory are in place so Plan 2/P2 can add
  vector/hybrid strategies and feed the catalog from live upstream MCP servers without touching
  the BM25 core.

## Hand-off to Plan 2 (live MCP I/O layer)

Plan 2 will add, against the same `Catalog` + `RetrievalStrategy` interfaces:
- `crates/upstream`: `rmcp` client connections (stdio + Streamable HTTP), lifecycle/health/reconnect,
  `tools/list` ingestion into the `Catalog`, and `notifications/tools/list_changed` → re-index.
- `crates/downstream`: `rmcp` server exposing the three meta-tools over stdio + axum HTTP.
- `crates/metatools`: `search_tools` / `get_tool_details` / `call_tool` wiring catalog + retrieval + router.
- `crates/config`: extend with `[server]` and `[[upstream]]` sections (already reserved in the spec).

### Deferred notes from the Plan 1 final review (address during Plan 2)

These were judged non-blocking for Plan 1 but should be revisited as the live I/O layer lands:

- **Dual strategy whitelist.** `config::validate` accepts `["bm25","vector","hybrid"]` while
  `retrieval::build_strategy` only implements `bm25`. The lists can drift — when implementing
  vector/hybrid, make the "implemented?" check the single source of truth (e.g. have config defer
  to the retrieval layer, or add a comment cross-linking them).
- **CLI default catalog path.** `mcpgw`'s `--catalog` defaults to the dev fixture
  `tests/fixtures/tools.json` (CWD-relative). Before anything user-facing ships, make `--catalog`
  required or env/config-driven.
- **`index(&mut self)` vs concurrency.** The live server will refresh the catalog while serving
  searches. `RetrievalStrategy::index(&mut self)` forces a write lock over the whole strategy during
  re-index. Consider a build-then-swap shape (build an immutable index, swap behind `ArcSwap`) so
  reads aren't blocked during refresh.
- **Silent dedup on catalog load.** `Catalog::upsert` (and thus `from_json_str`) silently
  last-wins on duplicate `{server}__{name}`. When ingesting tool lists from live upstreams,
  add explicit duplicate/collision detection (warn or error).
