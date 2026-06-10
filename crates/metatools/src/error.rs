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
