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
