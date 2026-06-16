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

/// One `search_tools` hit: the namespaced tool name, its one-line description, and the
/// retrieval relevance `score` (higher is better; hits are returned in descending score order).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    pub score: f32,
}
