# mcpgw M1-B.1 — Gateway State + Meta-tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the "gateway brain": a `metatools` crate (the 3 meta-tool functions over an immutable snapshot) and a `gateway` crate (an `ArcSwap` snapshot of catalog+strategy with `rebuild_snapshot`), plus the per-call timeout the meta-tools rely on — all testable with the in-memory mock upstream.

**Architecture:** `metatools` defines `GatewaySnapshot { catalog, strategy }` + `search_tools`/`get_tool_details`/`call_tool` (routing via catalog lookup, NOT `__` string-splitting). `gateway` holds `GatewayState { Arc<ArcSwap<GatewaySnapshot>>, UpstreamRegistry }` and rebuilds the snapshot (build-then-swap) from the registry's upstreams. `UpstreamHandle` gains a `call_timeout` applied inside `call_tool`. No rmcp server here — that (and real subprocess spawn / `serve`) is M1-B.2.

**Tech Stack:** Rust (edition 2021), `rmcp 1.7` (reused via `upstream`), `arc-swap = "1"`, `tokio`, `serde_json`, `thiserror`. Reuses `catalog`, `retrieval`, `upstream` (incl. its `testkit` mock).

> **Spec:** `docs/superpowers/specs/2026-06-09-mcpgw-m1b-gateway-design.md` (M1-B.1 section).
> **Docs DoD:** per `docs/README.md`, each task updates the matching L1–L4 docs in the same commit; Task 6 consolidates the new crates' docs.
> **Scope note (refinement of the spec):** real stdio subprocess spawn (`connect_stdio_upstream`/`connect_all`) and the `serve` command move to **M1-B.2** so every B.1 task is automatically testable with the in-memory mock. `GatewayState` exposes its `UpstreamRegistry` so B.2's eager-connect can populate it.

---

## File Structure

```
crates/upstream/src/connection.rs    # + call_timeout field, with_call_timeout(), timeout-wrapped call_tool; UpstreamError::Timeout
crates/upstream/src/testkit.rs       # + a `slow` tool (sleeps) for timeout tests
crates/metatools/
├─ Cargo.toml
├─ src/lib.rs        # re-exports + crate docs
├─ src/snapshot.rs   # GatewaySnapshot { catalog, strategy } + ToolSummary
├─ src/error.rs      # MetaError
└─ src/tools.rs      # search_tools / get_tool_details / call_tool
   + tests/call_tool.rs   # integration test (mock upstream + registry), required-features
crates/gateway/
├─ Cargo.toml
├─ src/lib.rs        # GatewayState + rebuild_snapshot
└─ tests/rebuild.rs  # integration test (inject mock handles -> rebuild -> search), required-features
Cargo.toml           # workspace: + arc-swap dep; + crates/metatools, crates/gateway members
docs/...             # L1-L4 updates (Task 6)
```

---

## Task 1: `UpstreamHandle` per-call timeout (+ testkit `slow` tool)

**Files:**
- Modify: `crates/upstream/src/connection.rs`
- Modify: `crates/upstream/src/testkit.rs`
- Modify: `crates/upstream/tests/integration.rs`

- [ ] **Step 1: Add a `slow` tool to the mock upstream**

In `crates/upstream/src/testkit.rs`, inside `#[tool_router] impl MockUpstream`, add (next to `echo`/`greet`):

```rust
    #[tool(description = "Sleep 10s then return (for timeout tests)")]
    async fn slow(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        Ok(CallToolResult::success(vec![Content::text("done")]))
    }
```

- [ ] **Step 2: Write the failing timeout test**

Append to `crates/upstream/tests/integration.rs`:

```rust
#[tokio::test]
async fn call_tool_times_out_when_slower_than_call_timeout() {
    let (handle, server) = connect_mock("mock").await;
    let handle = handle.with_call_timeout(std::time::Duration::from_millis(50));

    let err = handle.call_tool("slow", None).await.unwrap_err();
    assert!(
        matches!(err, upstream::connection::UpstreamError::Timeout { .. }),
        "expected Timeout, got {err:?}"
    );

    server.abort();
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p upstream --features testkit --test integration call_tool_times_out_when_slower_than_call_timeout`
Expected: FAIL — `no method named with_call_timeout` / `no variant Timeout`.

- [ ] **Step 4: Implement the timeout**

In `crates/upstream/src/connection.rs`:

(a) Add a `Timeout` variant to `UpstreamError` (inside the existing enum):

```rust
    #[error("upstream {server:?} call timed out")]
    Timeout { server: String },
```

(b) Add a `call_timeout` field to the struct and a default + builder. Change the struct and `connect` to set a default, and add `with_call_timeout`:

```rust
pub struct UpstreamHandle {
    server: String,
    client: RunningService<RoleClient, ()>,
    call_timeout: std::time::Duration,
}
```

In `connect`, set the field when constructing `Self`:

```rust
        Ok(Self {
            server: server.to_string(),
            client,
            call_timeout: std::time::Duration::from_secs(30),
        })
```

Add the builder (in the same `impl UpstreamHandle`):

```rust
    /// Set the per-call timeout (consumed before the handle is shared via `Arc`).
    pub fn with_call_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.call_timeout = timeout;
        self
    }
```

(c) Wrap `call_tool` in the timeout:

```rust
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult, UpstreamError> {
        let mut params = CallToolRequestParams::new(tool.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }
        let fut = self.client.call_tool(params);
        match tokio::time::timeout(self.call_timeout, fut).await {
            Err(_elapsed) => Err(UpstreamError::Timeout {
                server: self.server.clone(),
            }),
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(UpstreamError::Call {
                server: self.server.clone(),
                source: Box::new(e),
            }),
        }
    }
```

- [ ] **Step 5: Run the test + the existing suite**

Run: `cargo test -p upstream --features testkit`
Expected: all upstream tests pass, including the new timeout test (existing tests use the default 30s timeout, unaffected).
Run: `cargo clippy -p upstream --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/upstream/src/connection.rs crates/upstream/src/testkit.rs crates/upstream/tests/integration.rs
git commit -m "feat(upstream): per-call timeout on UpstreamHandle::call_tool"
```

---

## Task 2: `metatools` crate scaffold + `GatewaySnapshot` / `ToolSummary` / `MetaError`

**Files:**
- Modify: `Cargo.toml` (workspace)
- Create: `crates/metatools/Cargo.toml`
- Create: `crates/metatools/src/lib.rs`
- Create: `crates/metatools/src/snapshot.rs`
- Create: `crates/metatools/src/error.rs`

- [ ] **Step 1: Register the crate + add arc-swap to the workspace**

Edit root `Cargo.toml`: add `crates/metatools` to `members`, and add to `[workspace.dependencies]`:

```toml
arc-swap = "1"
```

So `members` includes `"crates/metatools"` (keep the existing members).

- [ ] **Step 2: Create `crates/metatools/Cargo.toml`**

```toml
[package]
name = "metatools"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
catalog = { path = "../catalog" }
retrieval = { path = "../retrieval" }
upstream = { path = "../upstream" }
rmcp = { workspace = true, features = ["client", "server", "macros", "transport-child-process", "transport-io"] }
serde_json = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
upstream = { path = "../upstream", features = ["testkit"] }
tokio = { workspace = true, features = ["full"] }
```

> The `[dev-dependencies] upstream = { features = ["testkit"] }` makes `upstream::testkit::MockUpstream`
> available to this crate's tests (the dev-dep enables the feature for the test build); no `required-features`
> gating is needed, and the production `metatools` lib (built from `[dependencies]` only) excludes testkit.

- [ ] **Step 3: Create the types and a snapshot constructor**

Create `crates/metatools/src/error.rs`:

```rust
//! Errors surfaced by the meta-tool functions (mapped to MCP `isError` by the downstream server).

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("no such tool: {0}")]
    ToolNotFound(String),
    #[error("upstream {0:?} is unavailable")]
    UpstreamUnavailable(String),
    #[error("upstream call timed out")]
    Timeout,
    #[error("upstream call failed: {0}")]
    Call(String),
}
```

Create `crates/metatools/src/snapshot.rs`:

```rust
//! The immutable gateway snapshot (catalog + built retrieval strategy) and the
//! `search_tools` result item.

use catalog::Catalog;
use retrieval::RetrievalStrategy;

/// An immutable snapshot of the aggregated tool catalog plus an indexed retrieval
/// strategy over it. Held behind an `ArcSwap` by the `gateway` crate.
pub struct GatewaySnapshot {
    pub(crate) catalog: Catalog,
    pub(crate) strategy: Box<dyn RetrievalStrategy>,
}

impl GatewaySnapshot {
    /// Build a snapshot from a catalog and an already-indexed strategy.
    pub fn new(catalog: Catalog, strategy: Box<dyn RetrievalStrategy>) -> Self {
        Self { catalog, strategy }
    }
}

/// One `search_tools` hit: the namespaced tool name and its one-line description.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
}
```

Create `crates/metatools/src/lib.rs`:

```rust
//! mcpgw `metatools`: the three meta-tool functions (`search_tools`, `get_tool_details`,
//! `call_tool`) over an immutable `GatewaySnapshot`. The downstream MCP server (M1-B.2)
//! exposes these as MCP tools.

pub mod error;
pub mod snapshot;
pub mod tools;

pub use error::MetaError;
pub use snapshot::{GatewaySnapshot, ToolSummary};
```

> Note: `serde` is needed for `ToolSummary`'s derive. Add `serde = { workspace = true }` to
> `crates/metatools/Cargo.toml` `[dependencies]` (alongside the others).

Add to `crates/metatools/Cargo.toml` `[dependencies]`:

```toml
serde = { workspace = true }
```

- [ ] **Step 4: Create a placeholder `tools.rs` so the crate compiles**

Create `crates/metatools/src/tools.rs`:

```rust
//! The three meta-tool functions. Implemented across Tasks 3-4.
```

- [ ] **Step 5: Verify it builds**

Run: `cargo build -p metatools`
Expected: compiles (no tests yet).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/metatools
git commit -m "feat(metatools): crate scaffold + GatewaySnapshot/ToolSummary/MetaError"
```

---

## Task 3: `search_tools` + `get_tool_details`

**Files:**
- Modify: `crates/metatools/src/tools.rs`

- [ ] **Step 1: Write the failing unit tests**

Replace `crates/metatools/src/tools.rs` with:

```rust
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
```

- [ ] **Step 2: Run to verify fail-then-pass**

Run: `cargo test -p metatools search_tools_returns_namespaced_summaries get_tool_details_returns_full_def_or_none`
Expected: PASS (the functions are defined in the same change; if a `ScoredTool` field name differs, reconcile to `retrieval::ScoredTool { qualified_name, description, score }`).

- [ ] **Step 3: Commit**

```bash
git add crates/metatools/src/tools.rs
git commit -m "feat(metatools): search_tools + get_tool_details over the snapshot"
```

---

## Task 4: `call_tool` (routing via catalog lookup, with timeout + errors)

**Files:**
- Modify: `crates/metatools/src/tools.rs`
- Create: `crates/metatools/tests/call_tool.rs`

- [ ] **Step 1: Implement `call_tool`**

Add to `crates/metatools/src/tools.rs` (after `get_tool_details`, before `#[cfg(test)]`):

```rust
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
```

> Note: `snap.catalog` and `snap.strategy` are `pub(crate)` fields, accessible here because
> `tools.rs` is in the same crate as `snapshot.rs`.

- [ ] **Step 2: Write the failing integration test**

Create `crates/metatools/tests/call_tool.rs`:

```rust
use std::sync::Arc;

use catalog::Catalog;
use metatools::{call_tool, GatewaySnapshot, MetaError};
use retrieval::{Bm25Strategy, RetrievalStrategy};
use rmcp::ServiceExt;
use upstream::connection::UpstreamHandle;
use upstream::registry::UpstreamRegistry;
use upstream::testkit::MockUpstream;

/// Connect the in-memory mock under namespace `server`, ingest its tools into `catalog`,
/// and register the handle. Returns (snapshot, registry, server-join-handle).
async fn setup(server: &str) -> (GatewaySnapshot, UpstreamRegistry, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let join = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(server, client_io).await.unwrap();

    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();

    let registry = UpstreamRegistry::new();
    registry.insert(Arc::new(handle));

    let mut strat = Bm25Strategy::new();
    strat.index(&catalog);
    let snap = GatewaySnapshot::new(catalog, Box::new(strat));
    (snap, registry, join)
}

#[tokio::test]
async fn call_tool_routes_via_catalog_and_forwards() {
    let (snap, registry, join) = setup("mock").await;

    let mut args = serde_json::Map::new();
    args.insert("text".into(), serde_json::Value::String("ping".into()));
    let result = call_tool(&snap, &registry, "mock__echo", Some(args)).await.unwrap();

    assert_eq!(result.is_error, Some(false));
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .expect("text content");
    assert_eq!(text, "ping");

    join.abort();
}

#[tokio::test]
async fn call_tool_unknown_tool_is_tool_not_found() {
    let (snap, registry, join) = setup("mock").await;
    let err = call_tool(&snap, &registry, "mock__nope", None).await.unwrap_err();
    assert!(matches!(err, MetaError::ToolNotFound(_)), "got {err:?}");
    join.abort();
}
```

- [ ] **Step 3: Run the integration test**

Run: `cargo test -p metatools --test call_tool`
Expected: PASS — 2 tests. (If `CallToolResult` accessors differ, they were already locked in M1-A; reuse those forms.)

- [ ] **Step 4: Verify the no-`__`-split routing claim with a unit test**

Add to the `tests` module in `crates/metatools/src/tools.rs`:

```rust
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
```

- [ ] **Step 5: Run all metatools tests**

Run: `cargo test -p metatools`
Run: `cargo clippy -p metatools --all-targets --all-features -- -D warnings`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/metatools/src/tools.rs crates/metatools/tests/call_tool.rs
git commit -m "feat(metatools): call_tool routing via catalog lookup (no __ split) + timeout/error mapping"
```

---

## Task 5: `gateway` crate — `GatewayState` + `rebuild_snapshot`

**Files:**
- Modify: `Cargo.toml` (workspace)
- Create: `crates/gateway/Cargo.toml`
- Create: `crates/gateway/src/lib.rs`
- Create: `crates/gateway/tests/rebuild.rs`

- [ ] **Step 1: Register the crate**

Edit root `Cargo.toml`: add `crates/gateway` to `members`.

- [ ] **Step 2: Create `crates/gateway/Cargo.toml`**

```toml
[package]
name = "gateway"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
catalog = { path = "../catalog" }
retrieval = { path = "../retrieval" }
upstream = { path = "../upstream" }
metatools = { path = "../metatools" }
arc-swap = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
upstream = { path = "../upstream", features = ["testkit"] }
tokio = { workspace = true, features = ["full"] }
rmcp = { workspace = true, features = ["client", "server", "macros", "transport-io"] }
```

- [ ] **Step 3: Implement `GatewayState`**

Create `crates/gateway/src/lib.rs`:

```rust
//! mcpgw `gateway`: holds the live, atomically-swappable `GatewaySnapshot` plus the
//! registry of upstream connections, and rebuilds the snapshot from the upstreams.

use std::sync::Arc;

use arc_swap::ArcSwap;
use catalog::Catalog;
use metatools::GatewaySnapshot;
use retrieval::build_strategy;
use upstream::registry::UpstreamRegistry;

/// Shared, cheaply-cloneable gateway state: an `ArcSwap` snapshot (read lock-free) and the
/// upstream registry. `strategy_name` selects the retrieval strategy used on each rebuild.
#[derive(Clone)]
pub struct GatewayState {
    snapshot: Arc<ArcSwap<GatewaySnapshot>>,
    registry: UpstreamRegistry,
    strategy_name: Arc<str>,
}

impl GatewayState {
    /// Create empty state (no upstreams, empty catalog) using `strategy_name` (e.g. "bm25").
    /// Returns an error if the strategy is not implemented.
    pub fn new(strategy_name: &str) -> Result<Self, String> {
        let mut strat = build_strategy(strategy_name).map_err(|e| e.to_string())?;
        let empty = Catalog::new();
        strat.index(&empty);
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
        })
    }

    /// The upstream registry (B.2's eager-connect populates it; tests inject mock handles).
    pub fn registry(&self) -> &UpstreamRegistry {
        &self.registry
    }

    /// Load the current snapshot (lock-free).
    pub fn snapshot(&self) -> Arc<GatewaySnapshot> {
        self.snapshot.load_full()
    }

    /// Rebuild the snapshot from the current registry: ingest every upstream's tools into a
    /// fresh catalog, build+index the strategy, and atomically swap it in. A single upstream
    /// failing to ingest is isolated (warn + skip); others still appear.
    pub async fn rebuild_snapshot(&self) -> Result<(), String> {
        let mut catalog = Catalog::new();
        for name in self.registry.server_names() {
            if let Some(handle) = self.registry.get(&name) {
                if let Err(e) = handle.ingest_into(&mut catalog).await {
                    tracing::warn!(upstream = %name, error = %e, "ingest failed; skipping");
                }
            }
        }
        let mut strat = build_strategy(&self.strategy_name).map_err(|e| e.to_string())?;
        strat.index(&catalog);
        self.snapshot
            .store(Arc::new(GatewaySnapshot::new(catalog, strat)));
        Ok(())
    }
}
```

- [ ] **Step 4: Write the failing integration test**

Create `crates/gateway/tests/rebuild.rs`:

```rust
use std::sync::Arc;

use gateway::GatewayState;
use metatools::search_tools;
use rmcp::ServiceExt;
use upstream::connection::UpstreamHandle;
use upstream::testkit::MockUpstream;

async fn connect_mock(name: &str) -> (UpstreamHandle, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let join = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    (handle, join)
}

#[tokio::test]
async fn rebuild_snapshot_ingests_registered_upstreams() {
    let state = GatewayState::new("bm25").unwrap();

    // Empty before any upstream.
    assert!(search_tools(&state.snapshot(), "echo", 5).is_empty());

    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    // After rebuild, the mock's namespaced tools are searchable.
    let hits = search_tools(&state.snapshot(), "echo", 5);
    assert!(hits.iter().any(|s| s.name == "mock__echo"), "hits: {hits:?}");

    join.abort();
}

#[tokio::test]
async fn old_snapshot_reader_is_unaffected_by_rebuild() {
    let state = GatewayState::new("bm25").unwrap();
    let old = state.snapshot(); // hold a guard to the empty snapshot

    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    // The previously-loaded snapshot still works and still reflects the OLD (empty) state.
    assert!(search_tools(&old, "echo", 5).is_empty());
    // A freshly-loaded snapshot reflects the new state.
    assert!(!search_tools(&state.snapshot(), "echo", 5).is_empty());

    join.abort();
}
```

- [ ] **Step 5: Run + lint**

Run: `cargo test -p gateway --test rebuild`
Expected: PASS — 2 tests.
Run: `cargo clippy -p gateway --all-targets --all-features -- -D warnings`
Expected: clean.

> Note on `search_tools(&state.snapshot(), ...)`: `snapshot()` returns `Arc<GatewaySnapshot>` (via
> `ArcSwap::load_full`); `&arc` deref-coerces to `&GatewaySnapshot`. Each `rebuild_snapshot` stores a new
> `Arc`, so a previously-returned `Arc` keeps pointing at the old snapshot (the `old_snapshot_reader` test
> relies on exactly this).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/gateway
git commit -m "feat(gateway): GatewayState with ArcSwap snapshot + rebuild_snapshot"
```

---

## Task 6: L1–L4 docs + finalize

**Files:**
- Create: `docs/L2-components/metatools.md`, `docs/L2-components/gateway.md`
- Create: `docs/L3-details/metatools.md`, `docs/L3-details/gateway.md`
- Create: `docs/L4-api/metatools-tools.md`, `docs/L4-api/metatools-snapshot.md`, `docs/L4-api/gateway-lib.md`
- Modify: `docs/L1-overview.md`, `docs/README.md`, and the `upstream` L4 doc for the new timeout

- [ ] **Step 1: Write the docs (READ the final code first)**

- `docs/L2-components/metatools.md` — 职责（三个元工具函数 over `GatewaySnapshot`）、接口表
  (`search_tools`/`get_tool_details`/`call_tool`, `GatewaySnapshot::new`, `ToolSummary`, `MetaError`)、
  依赖、被谁用（downstream/gateway）、不变量（路由经 catalog 查、不拆 `__`）。
- `docs/L2-components/gateway.md` — 职责（ArcSwap 快照状态 + rebuild）、接口表
  (`GatewayState::{new, registry, snapshot, rebuild_snapshot}`)、依赖、不变量（build-then-swap、读无锁、
  rebuild 隔离单上游失败）。
- `docs/L3-details/metatools.md` — `GatewaySnapshot` 不可变性、`search`→`ToolSummary` 映射、call_tool
  的 catalog 路由细节（含 tool 名带 `__` 的反例）、`MetaError`→（B.2）MCP isError 的关系、超时来自
  `UpstreamHandle`。
- `docs/L3-details/gateway.md` — `ArcSwap` 语义（`from_pointee`/`load`/`store`、读端 Guard、旧读者安全）、
  `rebuild_snapshot` 的 ingest→build→swap 流程与隔离、`strategy_name` 作用、`connect_all`/`serve` 属 B.2。
- 三个 `docs/L4-api/*.md` — 逐 pub item 精确签名（`tools.rs`、`snapshot.rs`、`gateway/lib.rs`）。
- 更新 `docs/L1-overview.md`：把 `metatools`、`gateway` 加入架构与依赖图，标注 M1-B.1 done、B.2 待做。
- 更新 `docs/README.md`：补 L2/L3/L4 索引链接。
- 更新 `docs/L4-api/upstream-connection.md`：`UpstreamHandle` 新增 `with_call_timeout`、`call_tool` 现带
  超时、`UpstreamError::Timeout` 变体。

- [ ] **Step 2: Format, lint, test the whole workspace**

Run: `cargo fmt --all`
Run: `cargo clippy --all-targets --all-features -- -D warnings`  → clean
Run: `cargo test --all-features`  → all green (catalog, retrieval, config, mcpgw, upstream, metatools, gateway)
Run: `cargo test`  (default) → green; the `metatools`/`gateway`/`upstream` integration tests gated on `upstream/testkit` are skipped.

- [ ] **Step 3: Commit**

```bash
git add docs/ crates/
git commit -m "docs(metatools,gateway): L1-L4 docs for B.1; fmt/clippy/test clean"
```

---

## Self-Review (run before execution)

**Spec coverage (M1-B.1 slice):**
- §2.2 metatools types + 3 functions — Tasks 2,3,4. §2.3 GatewayState + rebuild_snapshot — Task 5.
  §2.4 UpstreamHandle timeout — Task 1. §2.6 errors (`MetaError`, isolation) — Tasks 4,5. §2.7 mock-only
  tests — every task. Docs DoD — Task 6.
- **Deferred to M1-B.2 (intentionally not here):** `connect_stdio_upstream`/`connect_all` (real spawn),
  `serve`, `[server]` config, the `downstream` rmcp server, list_changed. `GatewayState::registry()` is the
  seam B.2 uses to populate upstreams.

**Placeholder scan:** none. The "reconcile if a field differs" notes name exact already-locked types.

**Type consistency:** `GatewaySnapshot::new(Catalog, Box<dyn RetrievalStrategy>)`, `ToolSummary{name,description}`,
`MetaError::{ToolNotFound,UpstreamUnavailable,Timeout,Call}`, `search_tools(&GatewaySnapshot,&str,usize)->Vec<ToolSummary>`,
`get_tool_details(&GatewaySnapshot,&str)->Option<&ToolDef>`, `call_tool(&GatewaySnapshot,&UpstreamRegistry,&str,Option<Map>)->Result<CallToolResult,MetaError>`,
`GatewayState::{new,registry,snapshot,rebuild_snapshot}`, `UpstreamHandle::with_call_timeout`,
`UpstreamError::Timeout{server}` are used consistently across tasks. `retrieval::ScoredTool{qualified_name,description,score}`
and `upstream::registry::UpstreamRegistry`/`upstream::connection::{UpstreamHandle,UpstreamError}` match the merged code.
