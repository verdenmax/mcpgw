//! mcpgw `metatools`: the three meta-tool functions (`search_tools`, `get_tool_details`,
//! `call_tool`) over an immutable `GatewaySnapshot`. The downstream MCP server (M1-B.2)
//! exposes these as MCP tools.

pub mod error;
pub mod snapshot;
pub mod tools;

pub use error::MetaError;
pub use snapshot::{GatewaySnapshot, ToolSummary};
