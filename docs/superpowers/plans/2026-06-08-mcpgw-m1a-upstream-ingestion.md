# mcpgw M1-A — rmcp Foundation + Upstream + Ingestion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `upstream` crate: connect to upstream MCP servers (stdio) via `rmcp`, ingest their tools into a namespaced `Catalog`, and forward `call_tool` — with an in-memory mock-upstream test harness that locks the rmcp API.

**Architecture:** New `upstream` crate depends on `rmcp` (1.7) and `catalog`. It exposes a transport-agnostic `connect`/`UpstreamHandle` (so tests drive it over an in-memory `tokio::io::duplex` mock instead of real child processes), a `Tool → ToolDef` mapping with namespace conflict detection, and a `UpstreamRegistry` keyed by server name. `config` gains a typed `[[upstream]]` (stdio) section. This is M1-A of the M1 milestone; the gateway state + meta-tools + downstream server are M1-B, Streamable HTTP is M1-C.

**Tech Stack:** Rust (edition 2021), `rmcp = "1.7"` (features: `client`, `server`, `macros`, `transport-child-process`, `transport-io`), `tokio`, `serde`, `thiserror`, `tracing`. Reuses M0's `catalog::{Catalog, ToolDef}` and `config`.

> **Spec:** `docs/superpowers/specs/2026-06-08-mcpgw-m1-live-gateway-design.md`.
> **Docs DoD:** Per `docs/README.md`, every task updates the matching L1–L4 docs in the same commit.
> Task 8 consolidates the `upstream` crate's L1–L4 docs.

> **About Task 1 (the spike):** rmcp 1.7's exact macro/transport forms can only be fully confirmed by
> compiling against the installed crate. Task 1's explicit job is to make a real rmcp client↔server
> handshake compile and pass over `tokio::io::duplex`. **If a precise rmcp symbol/macro form differs
> from what is written here, adjust it to the installed rmcp 1.7 API until the test passes** — that is
> the deliverable. All later tasks build on the forms Task 1 locks.

---

## File Structure

```
crates/upstream/
├─ Cargo.toml
├─ src/
│  ├─ lib.rs            # re-exports; crate docs
│  ├─ testkit.rs        # in-memory mock upstream MCP server (cfg(any(test, feature="testkit")))
│  ├─ mapping.rs        # rmcp Tool -> catalog::ToolDef; ingest_tools() + conflict detection
│  ├─ connection.rs     # connect_stdio / connect_transport; UpstreamHandle (list_tools, call_tool)
│  └─ registry.rs       # UpstreamState, UpstreamRegistry (keyed by server name)
└─ tests/
   └─ integration.rs    # end-to-end over the duplex mock: ingest + forward + isolation
crates/config/src/lib.rs   # + UpstreamConfig, transport enum, [[upstream]] parsing & validation
Cargo.toml                 # add crates/upstream to members; rmcp in [workspace.dependencies]
docs/L2-components/upstream.md, docs/L3-details/upstream.md, docs/L4-api/upstream-*.md, docs/L1-overview.md (update)
```

---

## Task 1: `upstream` crate scaffold + rmcp spike (in-memory mock upstream)

**Files:**
- Modify: `Cargo.toml` (workspace)
- Create: `crates/upstream/Cargo.toml`
- Create: `crates/upstream/src/lib.rs`
- Create: `crates/upstream/src/testkit.rs`

- [ ] **Step 1: Add rmcp to workspace deps and register the crate**

Edit the root `Cargo.toml`: add `crates/upstream` to `members`, and add to `[workspace.dependencies]`:

```toml
rmcp = { version = "1.7", default-features = false }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-util", "process", "sync", "time"] }
tracing = "0.1"
```

So `members` becomes:
```toml
members = ["crates/catalog", "crates/retrieval", "crates/config", "crates/mcpgw", "crates/upstream"]
```

- [ ] **Step 2: Create `crates/upstream/Cargo.toml`**

```toml
[package]
name = "upstream"
version = "0.1.0"
edition = { workspace = true }

[features]
# Exposes the in-memory mock upstream for use by other crates' tests.
testkit = []

[dependencies]
catalog = { path = "../catalog" }
rmcp = { workspace = true, features = ["client", "server", "macros", "transport-child-process", "transport-io"] }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
schemars = "1"

[dev-dependencies]
tokio = { workspace = true, features = ["full"] }
```

- [ ] **Step 3: Create the in-memory mock upstream + the spike test**

Create `crates/upstream/src/testkit.rs`:

```rust
//! In-memory mock upstream MCP server for tests. Reusable by other crates via the
//! `testkit` feature. Exposes two tools: `echo` and `greet`.
#![cfg(any(test, feature = "testkit"))]

use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{tool, tool_router, tool_handler, ServerHandler};
use rmcp::model::{CallToolResult, Content, ServerInfo, ServerCapabilities, Implementation};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EchoParams {
    /// Text to echo back.
    pub text: String,
}

#[derive(Clone)]
pub struct MockUpstream {
    tool_router: ToolRouter<MockUpstream>,
}

#[tool_router]
impl MockUpstream {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "Echo the provided text back")]
    fn echo(&self, Parameters(EchoParams { text }): Parameters<EchoParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Greet the world")]
    fn greet(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::success(vec![Content::text("hello")]))
    }
}

impl Default for MockUpstream {
    fn default() -> Self { Self::new() }
}

#[tool_handler]
impl ServerHandler for MockUpstream {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }
}
```

Create `crates/upstream/src/lib.rs`:

```rust
//! mcpgw `upstream`: connect to upstream MCP servers, ingest their tools into a
//! namespaced catalog, and forward tool calls.

pub mod testkit;

#[cfg(test)]
mod spike_tests {
    use super::testkit::MockUpstream;
    use rmcp::ServiceExt;

    /// Spike: stand up the mock upstream as an rmcp server over an in-memory duplex,
    /// connect a client, and confirm the client sees the mock's two tools.
    /// This locks the rmcp 1.7 client+server API used by the rest of the crate.
    #[tokio::test]
    async fn client_lists_mock_upstream_tools_over_duplex() {
        let (server_io, client_io) = tokio::io::duplex(4096);

        let server = tokio::spawn(async move {
            let svc = MockUpstream::new().serve(server_io).await.unwrap();
            svc.waiting().await.unwrap();
        });

        let client = ().serve(client_io).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(names.contains(&"echo".to_string()), "tools were: {names:?}");
        assert!(names.contains(&"greet".to_string()), "tools were: {names:?}");

        client.cancel().await.unwrap();
        server.abort();
    }
}
```

- [ ] **Step 4: Run the spike test (adjust rmcp forms until it passes)**

Run: `cargo test -p upstream --features testkit client_lists_mock_upstream_tools_over_duplex -- --nocapture`
Expected: PASS — the client lists `echo` and `greet`.
If it does not compile, reconcile the exact rmcp 1.7 symbols (e.g. macro attribute spelling, `ErrorData` path, `serve`/transport import) against `cargo doc -p rmcp --open` or the installed source, until it passes. **Document any deviations from this plan in the commit message.**

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/upstream
git commit -m "feat(upstream): crate scaffold + rmcp spike with in-memory mock upstream"
```

---

## Task 2: `config` — `[[upstream]]` (stdio) section

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/config/src/lib.rs`:

```rust
    #[test]
    fn parses_stdio_upstreams() {
        let cfg = Config::from_toml_str(
            r#"
            [[upstream]]
            name = "github"
            transport = "stdio"
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-github"]
            env_passthrough = ["GITHUB_TOKEN"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.upstreams.len(), 1);
        let u = &cfg.upstreams[0];
        assert_eq!(u.name, "github");
        assert_eq!(u.call_timeout_ms, 30_000); // default
        match &u.transport {
            UpstreamTransport::Stdio { command, args, env_passthrough } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "@modelcontextprotocol/server-github"]);
                assert_eq!(env_passthrough, &["GITHUB_TOKEN"]);
            }
        }
    }

    #[test]
    fn rejects_upstream_name_with_double_underscore() {
        let err = Config::from_toml_str(
            "[[upstream]]\nname = \"a__b\"\ntransport = \"stdio\"\ncommand = \"x\"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_duplicate_upstream_names() {
        let err = Config::from_toml_str(
            "[[upstream]]\nname=\"a\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
             [[upstream]]\nname=\"a\"\ntransport=\"stdio\"\ncommand=\"y\"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn upstreams_default_to_empty() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.upstreams.is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p config parses_stdio_upstreams`
Expected: FAIL — `no field 'upstreams'` / `cannot find type UpstreamTransport`.

- [ ] **Step 3: Implement the config types**

In `crates/config/src/lib.rs`, replace the `Config` struct with the version that adds `upstreams`
(the `rename` maps the `[[upstream]]` array-of-tables key onto the `upstreams` field):

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default, rename = "upstream")]
    pub upstreams: Vec<UpstreamConfig>,
}
```

Add the upstream types (after `RetrievalConfig`):

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UpstreamConfig {
    /// Namespace prefix for this server's tools. Must not contain "__".
    pub name: String,
    /// Per-call timeout in milliseconds.
    #[serde(default = "default_call_timeout_ms")]
    pub call_timeout_ms: u64,
    #[serde(flatten)]
    pub transport: UpstreamTransport,
}

fn default_call_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum UpstreamTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env_passthrough: Vec<String>,
    },
}
```

> **Important (serde):** `UpstreamConfig` flattens the internally-tagged `UpstreamTransport`, so it must
> **NOT** carry `#[serde(deny_unknown_fields)]` — `flatten` + `deny_unknown_fields` is a known-incompatible
> serde combination and will break parsing. (The top-level `Config` keeps `deny_unknown_fields`; that is
> fine because it does not flatten anything.)

Extend `validate` (in `impl Config`) to check upstreams. Replace the existing `validate` body's end so it also runs:

```rust
        let mut seen = std::collections::HashSet::new();
        for u in &self.upstreams {
            if u.name.is_empty() {
                return Err(ConfigError::Invalid("upstream.name must not be empty".into()));
            }
            if u.name.contains("__") {
                return Err(ConfigError::Invalid(format!(
                    "upstream.name {:?} must not contain \"__\" (namespace separator)",
                    u.name
                )));
            }
            if !seen.insert(u.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate upstream.name {:?}",
                    u.name
                )));
            }
        }
        Ok(())
```

(Keep the existing strategy/top_k checks before this block; this replaces the final `Ok(())`.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p config`
Expected: all config tests pass (existing 6 + 4 new = 10).

- [ ] **Step 5: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): parse and validate [[upstream]] (stdio) sections"
```

---

## Task 3: `Tool → ToolDef` mapping + namespace conflict detection

**Files:**
- Create: `crates/upstream/src/mapping.rs`
- Modify: `crates/upstream/src/lib.rs` (add `pub mod mapping;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/upstream/src/mapping.rs`:

```rust
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
```

> Note: confirm `Tool::new(name, description, Arc<JsonObject>)` and the public field names
> (`name`, `description`, `input_schema`) against the rmcp 1.7 `model::Tool` locked in Task 1. If
> `Tool::new`'s signature differs, build the `Tool` value via its actual constructor/builder — the
> assertions on `tool_to_def`/`ingest_tools` stay the same.

- [ ] **Step 2: Wire the module**

Add to `crates/upstream/src/lib.rs` (near the top, after the crate doc):

```rust
pub mod mapping;
```

- [ ] **Step 3: Run to verify it fails then passes**

Run: `cargo test -p upstream --features testkit mapping`
Expected: FAIL first if symbols mismatch (fix per the note), then PASS — 3 mapping tests.

- [ ] **Step 4: Commit**

```bash
git add crates/upstream/src/mapping.rs crates/upstream/src/lib.rs
git commit -m "feat(upstream): map rmcp Tool -> ToolDef with namespace + dupe detection"
```

---

## Task 4: `UpstreamHandle` — connect (transport-generic) + ingest

**Files:**
- Create: `crates/upstream/src/connection.rs`
- Modify: `crates/upstream/src/lib.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/upstream/tests/integration.rs`:

```rust
use catalog::Catalog;
use upstream::testkit::MockUpstream;
use upstream::connection::UpstreamHandle;

use rmcp::ServiceExt;

/// Spawn the mock upstream over a duplex and return a connected UpstreamHandle.
async fn connect_mock(name: &str) -> (UpstreamHandle, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    (handle, server)
}

#[tokio::test]
async fn ingests_namespaced_tools_from_upstream() {
    let (handle, server) = connect_mock("mock").await;
    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();

    assert!(catalog.get("mock__echo").is_some());
    assert!(catalog.get("mock__greet").is_some());

    handle.shutdown().await;
    server.abort();
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p upstream --features testkit --test integration ingests_namespaced_tools_from_upstream`
Expected: FAIL — `cannot find ... UpstreamHandle`.

- [ ] **Step 3: Implement `connection.rs`**

Create `crates/upstream/src/connection.rs`:

```rust
//! A live connection to one upstream MCP server.

use catalog::Catalog;
use rmcp::service::{RoleClient, RunningService};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::ServiceExt;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::mapping::ingest_tools;

/// Errors from upstream operations.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("failed to connect to upstream {server:?}: {source}")]
    Connect { server: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
    #[error("upstream {server:?} call failed: {source}")]
    Call { server: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
}

/// A connected upstream MCP server: its namespace name + the running rmcp client.
pub struct UpstreamHandle {
    server: String,
    client: RunningService<RoleClient, ()>,
}

impl UpstreamHandle {
    /// Connect over any async-rw transport (real stdio child or an in-memory duplex).
    pub async fn connect<T>(server: &str, transport: T) -> Result<Self, UpstreamError>
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let client = ().serve(transport).await.map_err(|e| UpstreamError::Connect {
            server: server.to_string(),
            source: Box::new(e),
        })?;
        Ok(Self { server: server.to_string(), client })
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// Fetch this server's tools and ingest them (namespaced) into `catalog`.
    pub async fn ingest_into(&self, catalog: &mut Catalog) -> Result<(), UpstreamError> {
        let tools = self.client.list_all_tools().await.map_err(|e| UpstreamError::Call {
            server: self.server.clone(),
            source: Box::new(e),
        })?;
        ingest_tools(catalog, &self.server, &tools);
        Ok(())
    }

    /// Forward a tool call to this upstream. `tool` is the ORIGINAL (un-namespaced) name.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult, UpstreamError> {
        let mut params = CallToolRequestParams::new(tool.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }
        self.client.call_tool(params).await.map_err(|e| UpstreamError::Call {
            server: self.server.clone(),
            source: Box::new(e),
        })
    }

    /// Cancel the underlying rmcp service.
    pub async fn shutdown(self) {
        let _ = self.client.cancel().await;
    }
}
```

Add to `crates/upstream/src/lib.rs`:

```rust
pub mod connection;
```

- [ ] **Step 4: Run the test (reconcile rmcp types if needed)**

Run: `cargo test -p upstream --features testkit --test integration ingests_namespaced_tools_from_upstream`
Expected: PASS. If `RunningService<RoleClient, ()>`, `with_arguments`, or `CallToolRequestParams::new` differ in rmcp 1.7, adjust to the locked forms from Task 1; the test assertions are unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/upstream/src/connection.rs crates/upstream/src/lib.rs crates/upstream/tests/integration.rs
git commit -m "feat(upstream): UpstreamHandle connect + ingest namespaced tools"
```

---

## Task 5: `call_tool` forwarding (end-to-end over the mock)

**Files:**
- Modify: `crates/upstream/tests/integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/upstream/tests/integration.rs`:

```rust
#[tokio::test]
async fn forwards_call_tool_to_upstream() {
    let (handle, server) = connect_mock("mock").await;

    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("ping".into()));
    let result = handle.call_tool("echo", Some(args)).await.unwrap();

    // The mock's echo returns the text as a text content block.
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .unwrap_or_default();
    assert_eq!(text, "ping");
    assert_ne!(result.is_error, Some(true));

    handle.shutdown().await;
    server.abort();
}
```

- [ ] **Step 2: Run to verify it fails, then implement/adjust**

Run: `cargo test -p upstream --features testkit --test integration forwards_call_tool_to_upstream`
Expected: PASS using the `call_tool` implemented in Task 4. If `CallToolResult`'s content accessor differs (e.g. `as_text()` / `is_error` field name) in rmcp 1.7, adjust the assertion to the locked API (the behavior — echo returns "ping", not an error — is the contract).

- [ ] **Step 3: Commit**

```bash
git add crates/upstream/tests/integration.rs
git commit -m "test(upstream): forward call_tool to upstream echo end-to-end"
```

---

## Task 6: `UpstreamRegistry` + state

**Files:**
- Create: `crates/upstream/src/registry.rs`
- Modify: `crates/upstream/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/upstream/src/registry.rs`:

```rust
//! Registry of live upstream connections, keyed by server name, plus connection state.

use std::collections::HashMap;
use std::sync::Arc;

use crate::connection::UpstreamHandle;

/// Lifecycle state of an upstream connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamState {
    Connecting,
    Ready,
    Failed,
}

/// Thread-safe registry mapping server name -> connected handle.
#[derive(Clone, Default)]
pub struct UpstreamRegistry {
    inner: Arc<std::sync::RwLock<HashMap<String, Arc<UpstreamHandle>>>>,
}

impl UpstreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, handle: Arc<UpstreamHandle>) {
        self.inner.write().unwrap().insert(handle.server().to_string(), handle);
    }

    pub fn get(&self, server: &str) -> Option<Arc<UpstreamHandle>> {
        self.inner.read().unwrap().get(server).cloned()
    }

    pub fn remove(&self, server: &str) {
        self.inner.write().unwrap().remove(server);
    }

    pub fn server_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.inner.read().unwrap().keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_values_are_distinct() {
        assert_ne!(UpstreamState::Connecting, UpstreamState::Ready);
        assert_ne!(UpstreamState::Ready, UpstreamState::Failed);
    }
}
```

> The registry test that exercises `insert`/`get` with a real handle lives in the integration test
> (next step), because constructing an `UpstreamHandle` requires a live mock connection.

- [ ] **Step 2: Add an integration test for registry get/miss**

Append to `crates/upstream/tests/integration.rs`:

```rust
use upstream::registry::UpstreamRegistry;
use std::sync::Arc;

#[tokio::test]
async fn registry_returns_handle_by_name_and_none_for_missing() {
    let (handle, server) = connect_mock("mock").await;
    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(handle));

    assert_eq!(registry.server_names(), vec!["mock".to_string()]);
    assert!(registry.get("mock").is_some());
    assert!(registry.get("nope").is_none());

    // Forward a call through the registry-held handle.
    let h = registry.get("mock").unwrap();
    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("x".into()));
    let r = h.call_tool("echo", Some(args)).await.unwrap();
    assert_ne!(r.is_error, Some(true));

    server.abort();
}
```

> Note: `UpstreamHandle::shutdown(self)` consumes `self`, but the registry holds `Arc<UpstreamHandle>`.
> Keep `shutdown` for the non-Arc path; the registry simply drops its Arcs (rmcp cancels on drop).
> If a `&self` shutdown is needed later, add it in M1-B. (Do not add it now — YAGNI.)

- [ ] **Step 3: Wire + run**

Add to `crates/upstream/src/lib.rs`:

```rust
pub mod registry;
```

Run: `cargo test -p upstream --features testkit`
Expected: all unit + integration tests pass (spike, mapping x3, registry state, integration: ingest + forward + registry).

- [ ] **Step 4: Commit**

```bash
git add crates/upstream/src/registry.rs crates/upstream/src/lib.rs crates/upstream/tests/integration.rs
git commit -m "feat(upstream): UpstreamRegistry keyed by server name + state enum"
```

---

## Task 7: Multi-upstream isolation (one failure doesn't break others)

**Files:**
- Modify: `crates/upstream/tests/integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/upstream/tests/integration.rs`:

```rust
#[tokio::test]
async fn one_upstream_failure_does_not_block_others() {
    // Healthy upstream:
    let (good, good_server) = connect_mock("good").await;

    // "Failed" upstream: a transport with no server on the other end → connect errors,
    // but that must not stop us from using the healthy one.
    let (_dead_server_io, dead_client_io) = tokio::io::duplex(4096);
    drop(_dead_server_io); // no server will ever respond
    let bad = UpstreamHandle::connect("bad", dead_client_io).await;

    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(good));
    if let Ok(h) = bad {
        registry.insert(Arc::new(h));
    }
    // Whether or not "bad" connected, ingest of the good one still works:
    let mut catalog = catalog::Catalog::new();
    registry.get("good").unwrap().ingest_into(&mut catalog).await.unwrap();
    assert!(catalog.get("good__echo").is_some());

    good_server.abort();
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p upstream --features testkit --test integration one_upstream_failure_does_not_block_others`
Expected: PASS. (If connecting to a dead duplex hangs rather than errors, wrap the `connect` call site in this test with `tokio::time::timeout(Duration::from_secs(2), ...)` and treat elapsed as "failed" — the assertion that the healthy upstream still ingests is the contract.)

- [ ] **Step 3: Commit**

```bash
git add crates/upstream/tests/integration.rs
git commit -m "test(upstream): healthy upstream unaffected by a failed peer"
```

---

## Task 8: L1–L4 docs for `upstream` + finalize (fmt/clippy/test)

**Files:**
- Create: `docs/L2-components/upstream.md`
- Create: `docs/L3-details/upstream.md`
- Create: `docs/L4-api/upstream-mapping.md`, `docs/L4-api/upstream-connection.md`, `docs/L4-api/upstream-registry.md`
- Modify: `docs/L1-overview.md` (add the `upstream` crate to the architecture), `docs/README.md` (index links)

- [ ] **Step 1: Write the docs**

Create `docs/L2-components/upstream.md` — responsibility (connect to upstream MCP via rmcp, ingest
namespaced tools, forward calls), public interface table (`UpstreamHandle::{connect, ingest_into,
call_tool, server, shutdown}`, `UpstreamRegistry::{new, insert, get, remove, server_names}`,
`UpstreamState`, `mapping::{tool_to_def, ingest_tools}`, `UpstreamError`), dependencies (rmcp, catalog,
tokio), used-by (gateway in M1-B), invariants (namespace `{server}__{name}`; first-tool-wins on dupes).

Create `docs/L3-details/upstream.md` — rmcp 1.7 client usage (`().serve(transport)`,
`list_all_tools`, `call_tool`), transport-generic `connect<T: AsyncRead+AsyncWrite>` enabling
in-memory duplex tests, `Tool → ToolDef` field mapping (`description: Option<Cow>` → `""`,
`input_schema: Arc<JsonObject>` → `Value::Object`), conflict detection (warn + skip), registry
concurrency (`RwLock<HashMap>`; membership changes only on connect/disconnect), and the testkit mock.

Create the three `docs/L4-api/upstream-*.md` — per-file `pub` item signatures for `mapping.rs`,
`connection.rs`, `registry.rs`.

Update `docs/L1-overview.md`: add `upstream` to the crate list/architecture and note it depends on
`rmcp` + `catalog`. Update `docs/README.md` index to link the new L2/L3/L4 files.

- [ ] **Step 2: Format, lint, test the whole workspace**

Run: `cargo fmt --all`
Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean. Fix any findings.
Run: `cargo test --all-features`
Expected: all crates green (catalog, retrieval, config incl. new upstream tests, mcpgw, upstream).

- [ ] **Step 3: Commit**

```bash
git add docs/ crates/
git commit -m "docs(upstream): L1-L4 docs for the upstream crate; fmt/clippy/test clean"
```

---

## Self-Review (run before execution)

**Spec coverage (M1-A slice of the spec):**
- §4 `upstream` crate — Tasks 1,3,4,5,6,7. §3 rmcp choice — Task 1 spike. §5.1 ingestion/namespace/registry —
  Tasks 3,4,6. §8 `[[upstream]]` config — Task 2. §9 conflict detection (deferred ④) — Task 3.
  §10 mock-upstream harness + isolation — Tasks 1,7. Docs DoD — Task 8.
- **Deferred to M1-B/M1-C (intentionally not in this plan):** `[server]` config, downstream server,
  meta-tools, ArcSwap gateway state, `list_changed` subscription, Streamable HTTP, API-key, real
  stdio child-process connect helper (`connect_stdio` wrapping `TokioChildProcess`). The transport-
  generic `connect` makes adding the child-process wrapper a one-function task in M1-B.

**Placeholder scan:** No TBD/TODO. The "reconcile to rmcp 1.7" notes are scoped to the spike-locked
API and name exact symbols to check — not vague hand-waving.

**Type consistency:** `UpstreamHandle::{connect, ingest_into, call_tool, server, shutdown}`,
`UpstreamRegistry::{new, insert, get, remove, server_names}`, `UpstreamState`,
`mapping::{tool_to_def, ingest_tools}`, `UpstreamError`, `config::{UpstreamConfig, UpstreamTransport,
Config.upstreams}` are used consistently across tasks. The integration test reuses `connect_mock` and
`UpstreamRegistry` from earlier tasks.

> **Note for the implementer:** This plan front-loads rmcp API risk into Task 1. Treat Task 1 as a
> BLOCKING gate — if the rmcp client↔server duplex handshake cannot be made to pass, stop and escalate
> before proceeding; every later task depends on it.
