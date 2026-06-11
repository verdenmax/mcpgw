//! mcpgw `gateway`: holds the live, atomically-swappable `GatewaySnapshot` plus the
//! registry of upstream connections, and rebuilds the snapshot from the upstreams.

use std::sync::Arc;

use arc_swap::ArcSwap;
use catalog::Catalog;
use metatools::GatewaySnapshot;
use retrieval::build_strategy;
use tokio::sync::Mutex;
use upstream::registry::UpstreamRegistry;

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

/// Shared, cheaply-cloneable gateway state: an `ArcSwap` snapshot (read lock-free) and the
/// upstream registry. `strategy_name` selects the retrieval strategy used on each rebuild.
#[derive(Clone)]
pub struct GatewayState {
    snapshot: Arc<ArcSwap<GatewaySnapshot>>,
    registry: UpstreamRegistry,
    strategy_name: Arc<str>,
    /// Serializes rebuilds so concurrent triggers can't commit a stale snapshot
    /// (last-store-wins). Readers never touch this — they only load the `ArcSwap`.
    rebuild_lock: Arc<Mutex<()>>,
}

impl GatewayState {
    /// Create empty state (no upstreams, empty catalog) using `strategy_name` (e.g. "bm25").
    /// Returns an error if the strategy is not implemented.
    pub fn new(strategy_name: &str) -> Result<Self, GatewayError> {
        let mut strat =
            build_strategy(strategy_name).map_err(|e| GatewayError::Strategy(e.to_string()))?;
        let empty = Catalog::new();
        strat.index(&empty);
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
            rebuild_lock: Arc::new(Mutex::new(())),
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

    /// Rebuild the snapshot by ingesting every upstream's tools **concurrently**, each bounded
    /// by that handle's `call_timeout`. A slow/hung/failing upstream is isolated (recorded in
    /// `skipped`) and never blocks the others or the rebuild. Build-then-swap keeps reads
    /// lock-free; `rebuild_lock` serializes overlapping rebuilds (last-store-wins).
    pub async fn rebuild_snapshot(&self) -> Result<RebuildSummary, GatewayError> {
        let _guard = self.rebuild_lock.lock().await;

        let mut set = tokio::task::JoinSet::new();
        for name in self.registry.server_names() {
            if let Some(handle) = self.registry.get(&name) {
                let timeout = handle.call_timeout();
                set.spawn(async move {
                    let mut local = Catalog::new();
                    let outcome =
                        tokio::time::timeout(timeout, handle.ingest_into(&mut local)).await;
                    (name, outcome, local)
                });
            }
        }

        let mut summary = RebuildSummary::default();
        let mut catalog = Catalog::new();
        while let Some(joined) = set.join_next().await {
            let (name, outcome, local) = joined.expect("ingest task panicked");
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

        let mut strat = build_strategy(&self.strategy_name)
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        strat.index(&catalog);
        self.snapshot
            .store(Arc::new(GatewaySnapshot::new(catalog, strat)));
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
}
