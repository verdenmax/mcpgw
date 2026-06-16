//! Read-only web dashboard for mcpgw (subsystem A): metrics aggregation, discovery traces,
//! and a static SPA served over a separate localhost port.

mod metrics;
pub use metrics::{MetaToolMetrics, MetricsSink, MetricsSnapshot, UpstreamMetrics};

mod trace;
pub use trace::{DiscoveryRingSink, DiscoveryWriter};

mod history;
pub use history::{replay_audit_metrics, replay_discovery, MetricBucket};
