//! mcpgw `gateway`: holds the live, atomically-swappable `GatewaySnapshot` plus the
//! registry of upstream connections, and rebuilds the snapshot from the upstreams.

use std::collections::HashMap;
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
