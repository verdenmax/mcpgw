//! mcpgw `gateway`: holds the live, atomically-swappable `GatewaySnapshot` plus the
//! registry of upstream connections, and rebuilds the snapshot from the upstreams.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arc_swap::ArcSwap;
use arc_swap::ArcSwapOption;
use catalog::Catalog;
use metatools::GatewaySnapshot;
use retrieval::{build_strategy, Backends, Embedder};
use tokio::sync::Mutex;
use upstream::registry::UpstreamRegistry;

mod disable;
pub use disable::{DisableSet, DisabledSnapshot};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("unknown retrieval strategy: {0}")]
    Strategy(String),
}

/// Telemetry for one snapshot rebuild.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RebuildSummary {
    /// Upstreams whose tools were ingested into the new snapshot.
    pub ingested: Vec<String>,
    /// Upstreams skipped this rebuild, with a short reason (timeout / call error).
    pub skipped: Vec<(String, String)>,
}

/// One upstream-reconcile outcome (serialized into the dashboard's `ApplyResult`).
///
/// `added`/`reconnected` are *planned* intents: an entry that also appears in `connect_failures`
/// did not actually (re)connect — for a *changed* upstream the previous connection is retained.
/// Cross-reference `connect_failures` for the truly-applied set.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReconcileSummary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub reconnected: Vec<String>,
    pub connect_failures: Vec<(String, String)>, // (name, error)
}

/// Pure three-way diff of upstream configs by name: which to drop, which to (re)connect.
struct ReconcilePlan {
    removed: Vec<String>,
    to_connect: Vec<config::UpstreamConfig>,
    added: Vec<String>,
    reconnected: Vec<String>,
}

fn plan_upstream_reconcile(
    old: &[config::UpstreamConfig],
    new: &[config::UpstreamConfig],
) -> ReconcilePlan {
    let old_by: HashMap<&str, &config::UpstreamConfig> =
        old.iter().map(|u| (u.name.as_str(), u)).collect();
    let new_names: HashSet<&str> = new.iter().map(|u| u.name.as_str()).collect();

    let mut plan = ReconcilePlan {
        removed: vec![],
        to_connect: vec![],
        added: vec![],
        reconnected: vec![],
    };
    for u in old {
        if !new_names.contains(u.name.as_str()) {
            plan.removed.push(u.name.clone());
        }
    }
    for u in new {
        match old_by.get(u.name.as_str()) {
            None => {
                plan.added.push(u.name.clone());
                plan.to_connect.push(u.clone());
            }
            Some(prev) if *prev != u => {
                plan.reconnected.push(u.name.clone());
                plan.to_connect.push(u.clone());
            }
            Some(_) => {} // unchanged: leave the live connection untouched
        }
    }
    plan
}

/// Resolve one `JoinSet::join_next` result: the joined value to process, or `None` if the task
/// panicked/was cancelled. A `JoinError` is degraded to a skipped upstream (never re-panicked), so
/// a single bad ingest task can't crash the initial build or kill the rebuild worker. The
/// upstream is attributed via `names_by_id` (keyed by `task::Id`) when known.
fn resolve_joined<T>(
    joined: Result<T, tokio::task::JoinError>,
    names_by_id: &HashMap<tokio::task::Id, String>,
    summary: &mut RebuildSummary,
) -> Option<T> {
    match joined {
        Ok(v) => Some(v),
        Err(e) => {
            let name = names_by_id
                .get(&e.id())
                .cloned()
                .unwrap_or_else(|| "<ingest task>".to_string());
            tracing::warn!(upstream = %name, error = %e, "ingest task panicked/cancelled; skipping");
            summary.skipped.push((name, format!("task failed: {e}")));
            None
        }
    }
}

/// Shared, cheaply-cloneable gateway state: an `ArcSwap` snapshot (read lock-free) and the
/// upstream registry. `strategy_name` selects the retrieval strategy used on each rebuild.
#[derive(Clone)]
pub struct GatewayState {
    snapshot: Arc<ArcSwap<GatewaySnapshot>>,
    registry: UpstreamRegistry,
    strategy_name: Arc<str>,
    /// Optional retrieval backends (embedder/chat), held across rebuilds so a CachingEmbedder keeps its cache.
    backends: Backends,
    /// Serializes rebuilds so concurrent triggers can't commit a stale snapshot
    /// (last-store-wins). Readers never touch this — they only load the `ArcSwap`.
    rebuild_lock: Arc<Mutex<()>>,
    /// Most recent rebuild summary (ingested/skipped upstreams), for the dashboard. Read lock-free.
    last_summary: Arc<ArcSwapOption<RebuildSummary>>,
    /// Runtime disable set (default empty). Read on every rebuild to skip disabled upstreams/tools.
    disabled: Arc<disable::DisableSet>,
}

impl GatewayState {
    /// Assemble state for `strategy_name`, optionally backed by `backends`. Shared by
    /// `new`/`with_embedder`/`with_backends` so the assembly logic lives in one place (no drift).
    fn build(strategy_name: &str, backends: Backends) -> Result<Self, GatewayError> {
        let strat = build_strategy(strategy_name, &backends)
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        let empty = Catalog::new();
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
            backends,
            rebuild_lock: Arc::new(Mutex::new(())),
            last_summary: Arc::new(ArcSwapOption::empty()),
            disabled: Arc::new(disable::DisableSet::default()),
        })
    }

    /// Create empty state (no upstreams, empty catalog) using `strategy_name` (e.g. "bm25").
    /// Errors if the strategy is unknown or its required backend (embedder/chat) is missing.
    pub fn new(strategy_name: &str) -> Result<Self, GatewayError> {
        Self::build(strategy_name, Backends::default())
    }

    /// Create state whose retrieval strategy is backed by `embedder` (for "vector"/"hybrid").
    pub fn with_embedder(
        strategy_name: &str,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self, GatewayError> {
        Self::build(
            strategy_name,
            Backends {
                embedder: Some(embedder),
                ..Default::default()
            },
        )
    }

    /// Create state with arbitrary retrieval `backends` (e.g. a `chat` model for "subagent").
    pub fn with_backends(strategy_name: &str, backends: Backends) -> Result<Self, GatewayError> {
        Self::build(strategy_name, backends)
    }

    /// The upstream registry (B.2's eager-connect populates it; tests inject mock handles).
    pub fn registry(&self) -> &UpstreamRegistry {
        &self.registry
    }

    /// Replace the disable set (assembly-time injection, before the first rebuild). Cheap-clone
    /// `Arc` shared with the dashboard via the same `GatewayState`.
    pub fn with_disabled(mut self, disabled: Arc<disable::DisableSet>) -> Self {
        self.disabled = disabled;
        self
    }

    /// The runtime disable set (read by rebuild; mutated by the dashboard admin API).
    pub fn disabled(&self) -> &disable::DisableSet {
        self.disabled.as_ref()
    }

    /// An owned `Arc` clone of the disable set — for callers that move it across an `.await`
    /// (e.g. the dashboard admin handler runs the synchronous, fsync-ing mutation in `spawn_blocking`).
    pub fn disabled_arc(&self) -> Arc<disable::DisableSet> {
        self.disabled.clone()
    }

    /// Reconcile the upstream registry against a new config: drop removed upstreams, (re)connect
    /// added/changed ones (reusing the eager connect path), then rebuild the snapshot. Best-effort:
    /// a connect failure is recorded in `connect_failures` and never aborts the others or rolls back.
    pub async fn reconcile_upstreams(
        &self,
        old: &[config::UpstreamConfig],
        new: &[config::UpstreamConfig],
        trigger: upstream::connection::RebuildTrigger,
    ) -> ReconcileSummary {
        let plan = plan_upstream_reconcile(old, new);
        if plan.removed.is_empty() && plan.to_connect.is_empty() {
            return ReconcileSummary::default(); // nothing changed: skip remove/connect/rebuild
        }
        for name in &plan.removed {
            self.registry.remove(name);
        }
        let mut connect_failures = Vec::new();
        if !plan.to_connect.is_empty() {
            let csum =
                upstream::connect::connect_all(self.registry(), &plan.to_connect, trigger).await;
            connect_failures = csum.skipped;
        }
        let _ = self.rebuild_snapshot().await;
        ReconcileSummary {
            added: plan.added,
            removed: plan.removed,
            reconnected: plan.reconnected,
            connect_failures,
        }
    }

    /// Load the current snapshot (lock-free).
    pub fn snapshot(&self) -> Arc<GatewaySnapshot> {
        self.snapshot.load_full()
    }

    /// The most recent rebuild summary, or `None` before the first rebuild. Read lock-free.
    pub fn last_summary(&self) -> Option<Arc<RebuildSummary>> {
        self.last_summary.load_full()
    }

    /// Rebuild the snapshot by ingesting every upstream's tools **concurrently**, each bounded
    /// by that handle's `call_timeout`. A slow/hung/failing upstream is isolated (recorded in
    /// `skipped`) and never blocks the others or the rebuild. Build-then-swap keeps reads
    /// lock-free; `rebuild_lock` serializes overlapping rebuilds (last-store-wins).
    pub async fn rebuild_snapshot(&self) -> Result<RebuildSummary, GatewayError> {
        let _guard = self.rebuild_lock.lock().await;

        let mut set = tokio::task::JoinSet::new();
        let mut names_by_id: HashMap<tokio::task::Id, String> = HashMap::new();
        for name in self.registry.server_names() {
            if self.disabled.is_upstream_disabled(&name) {
                continue; // disabled upstream: not even ingested
            }
            if let Some(handle) = self.registry.get(&name) {
                let timeout = handle.call_timeout();
                let task_name = name.clone();
                let abort = set.spawn(async move {
                    let mut local = Catalog::new();
                    let outcome =
                        tokio::time::timeout(timeout, handle.ingest_into(&mut local)).await;
                    (name, outcome, local)
                });
                names_by_id.insert(abort.id(), task_name);
            }
        }

        let mut summary = RebuildSummary::default();
        let mut catalog = Catalog::new();
        while let Some(joined) = set.join_next().await {
            // A panicked/cancelled ingest task is degraded to a skipped upstream (see
            // `resolve_joined`) so crash isolation holds at both startup and in the rebuild worker.
            let Some((name, outcome, local)) = resolve_joined(joined, &names_by_id, &mut summary)
            else {
                continue;
            };
            match outcome {
                Err(_elapsed) => summary.skipped.push((name, "ingest timed out".to_string())),
                Ok(Err(e)) => summary.skipped.push((name, e.to_string())),
                Ok(Ok(_dupes)) => {
                    for tool in local.iter() {
                        if self.disabled.is_tool_disabled(&tool.qualified_name()) {
                            continue; // disabled single tool: skip upsert
                        }
                        catalog.upsert(tool.clone());
                    }
                    summary.ingested.push(name);
                }
            }
        }
        summary.ingested.sort();
        summary.skipped.sort();

        let mut strat = build_strategy(&self.strategy_name, &self.backends)
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        strat.index(&catalog).await;
        self.snapshot
            .store(Arc::new(GatewaySnapshot::new(catalog, strat)));
        self.last_summary.store(Some(Arc::new(summary.clone())));
        Ok(summary)
    }
}

/// Drain `rx` and rebuild the snapshot once per burst (coalescing consecutive triggers).
/// Exits when the channel closes (all `RebuildTrigger` senders dropped). `serve` spawns this.
pub async fn run_rebuild_worker(state: GatewayState, mut rx: tokio::sync::mpsc::Receiver<String>) {
    while rx.recv().await.is_some() {
        // Coalesce any other pending triggers so a burst yields a single rebuild.
        while rx.try_recv().is_ok() {}
        match state.rebuild_snapshot().await {
            Ok(s) => tracing::info!(
                ingested = ?s.ingested,
                skipped = ?s.skipped,
                "snapshot rebuilt (list_changed)"
            ),
            Err(e) => tracing::warn!(error = %e, "rebuild failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    /// `GatewayState` exists to be shared across async tasks (B.2's downstream server +
    /// connect manager hold cheap clones), so lock its `Send + Sync` at compile time.
    #[test]
    fn gateway_state_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::GatewayState>();
    }

    fn ups(toml: &str) -> Vec<config::UpstreamConfig> {
        config::Config::from_toml_str(toml).unwrap().upstreams
    }

    #[test]
    fn plan_reconcile_classifies_add_remove_change_unchanged() {
        let a = ups(
            "[[upstream]]\nname=\"keep\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
                     [[upstream]]\nname=\"drop\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
                     [[upstream]]\nname=\"chg\"\ntransport=\"stdio\"\ncommand=\"x\"\n",
        );
        let b = ups(
            "[[upstream]]\nname=\"keep\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
                     [[upstream]]\nname=\"chg\"\ntransport=\"stdio\"\ncommand=\"y\"\n\
                     [[upstream]]\nname=\"new\"\ntransport=\"stdio\"\ncommand=\"x\"\n",
        );
        let p = super::plan_upstream_reconcile(&a, &b);
        assert_eq!(p.removed, vec!["drop"]);
        assert_eq!(p.added, vec!["new"]);
        assert_eq!(p.reconnected, vec!["chg"]);
        // unchanged "keep" is neither removed nor reconnected nor in to_connect
        let tc: Vec<&str> = p.to_connect.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(tc, vec!["chg", "new"]); // order: new list order
        assert!(!tc.contains(&"keep"));
    }

    #[tokio::test]
    async fn with_embedder_rebuild_builds_vector_snapshot_no_upstreams() {
        let state = super::GatewayState::with_embedder(
            "vector",
            std::sync::Arc::new(retrieval::MockEmbedder::new(16)),
        )
        .expect("vector state");
        // No upstreams -> empty catalog; rebuild must succeed (embed of [] is fine).
        state.rebuild_snapshot().await.expect("rebuild ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn with_embedder_rebuild_builds_hybrid_snapshot_no_upstreams() {
        let state = super::GatewayState::with_embedder(
            "hybrid",
            std::sync::Arc::new(retrieval::MockEmbedder::new(16)),
        )
        .expect("hybrid state");
        // No upstreams -> empty catalog; rebuild must succeed (embed of [] is fine).
        state.rebuild_snapshot().await.expect("rebuild ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn with_backends_rebuild_builds_subagent_snapshot_no_upstreams() {
        // MockChatModel::new("[]") = chat selects no tools; no upstreams -> empty catalog;
        // rebuild must still succeed and search returns empty.
        let backends = retrieval::Backends {
            chat: Some(std::sync::Arc::new(retrieval::MockChatModel::new("[]"))),
            ..Default::default()
        };
        let state =
            super::GatewayState::with_backends("subagent", backends).expect("subagent state");
        state.rebuild_snapshot().await.expect("rebuild ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }

    /// A panicked ingest task (a real `JoinError`) must be degraded to a skipped upstream — never
    /// re-panicked — and attributed to its upstream via the `task::Id` map.
    #[tokio::test]
    async fn resolve_joined_attributes_panicked_task_to_its_upstream() {
        let h: tokio::task::JoinHandle<()> = tokio::spawn(async { panic!("boom") });
        let id = h.id();
        let join_err = h.await.expect_err("task must have panicked");
        assert!(join_err.is_panic());

        let mut names = std::collections::HashMap::new();
        names.insert(id, "boomserver".to_string());
        let mut summary = super::RebuildSummary::default();

        let resolved: Option<()> = super::resolve_joined(Err(join_err), &names, &mut summary);
        assert!(resolved.is_none(), "a JoinError must not yield a value");
        assert_eq!(summary.skipped.len(), 1);
        assert_eq!(summary.skipped[0].0, "boomserver");
        assert!(summary.skipped[0].1.contains("task failed"));
        assert!(summary.ingested.is_empty());
    }

    /// When the panicked task's id isn't in the map, it degrades to the generic name (still no panic).
    #[tokio::test]
    async fn resolve_joined_falls_back_to_generic_name_when_id_unknown() {
        let h: tokio::task::JoinHandle<()> = tokio::spawn(async { panic!("boom") });
        let join_err = h.await.expect_err("task must have panicked");

        let names = std::collections::HashMap::new();
        let mut summary = super::RebuildSummary::default();
        let resolved: Option<()> = super::resolve_joined(Err(join_err), &names, &mut summary);

        assert!(resolved.is_none());
        assert_eq!(summary.skipped.len(), 1);
        assert_eq!(summary.skipped[0].0, "<ingest task>");
    }

    #[tokio::test]
    async fn last_summary_is_none_until_first_rebuild() {
        let state = super::GatewayState::new("bm25").unwrap();
        assert!(state.last_summary().is_none());
        let _ = state.rebuild_snapshot().await.unwrap();
        let s = state
            .last_summary()
            .expect("summary recorded after rebuild");
        assert!(s.ingested.is_empty() && s.skipped.is_empty()); // no upstreams registered
    }

    /// The success path passes the joined value through unchanged.
    #[test]
    fn resolve_joined_passes_through_ok() {
        let names = std::collections::HashMap::new();
        let mut summary = super::RebuildSummary::default();
        let resolved = super::resolve_joined(Ok(42u32), &names, &mut summary);
        assert_eq!(resolved, Some(42));
        assert!(summary.skipped.is_empty());
    }
}
