//! Eager, degraded-start connection of all configured upstreams (real stdio children).

use std::sync::Arc;
use std::time::Duration;

use config::{UpstreamConfig, UpstreamTransport};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};

use crate::connection::{RebuildTrigger, UpstreamError, UpstreamHandle};
use crate::registry::UpstreamRegistry;

/// Outcome of `connect_all`: which upstreams connected vs. were skipped (with reason).
pub struct ConnectSummary {
    pub connected: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// Build the child command for a stdio upstream (env allow-list applied). The child's
/// environment is cleared and only the allow-listed vars present in mcpgw's own environment
/// are passed through. Exposed (crate-internal) for testing the allow-list behavior.
pub(crate) fn build_command(cfg: &UpstreamConfig) -> tokio::process::Command {
    let (command, args, env_passthrough) = match &cfg.transport {
        UpstreamTransport::Stdio {
            command,
            args,
            env_passthrough,
        } => (command, args, env_passthrough),
    };
    tokio::process::Command::new(command).configure(|c| {
        c.args(args);
        c.env_clear();
        for key in env_passthrough {
            if let Ok(val) = std::env::var(key) {
                c.env(key, val);
            }
        }
    })
}

/// Spawn one stdio upstream child and connect to it, applying `call_timeout_ms` and
/// installing the list_changed handler carrying `trigger`. The connect/initialize handshake
/// is bounded by `call_timeout_ms` so a child that spawns but never replies cannot hang
/// degraded start.
pub async fn connect_stdio_upstream(
    cfg: &UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError> {
    let cmd = build_command(cfg);
    let transport = TokioChildProcess::new(cmd).map_err(|e| UpstreamError::Connect {
        server: cfg.name.clone(),
        source: Box::new(e),
    })?;
    let connect = UpstreamHandle::connect_with_trigger(&cfg.name, transport, trigger);
    let handle =
        match tokio::time::timeout(Duration::from_millis(cfg.call_timeout_ms), connect).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                return Err(UpstreamError::Timeout {
                    server: cfg.name.clone(),
                })
            }
        };
    Ok(handle.with_call_timeout(Duration::from_millis(cfg.call_timeout_ms)))
}

/// Connect every configured upstream eagerly. Degraded start: a connect failure is
/// `warn!`-logged and recorded in `skipped`; successful handles are inserted into `registry`.
pub async fn connect_all(
    registry: &UpstreamRegistry,
    upstreams: &[UpstreamConfig],
    trigger: RebuildTrigger,
) -> ConnectSummary {
    let mut summary = ConnectSummary {
        connected: vec![],
        skipped: vec![],
    };
    for cfg in upstreams {
        match connect_stdio_upstream(cfg, Some(trigger.clone())).await {
            Ok(handle) => {
                registry.insert(Arc::new(handle));
                summary.connected.push(cfg.name.clone());
            }
            Err(e) => {
                tracing::warn!(upstream = %cfg.name, error = %e, "connect failed; skipping");
                summary.skipped.push((cfg.name.clone(), e.to_string()));
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{UpstreamConfig, UpstreamTransport};

    fn stdio_cfg(env_passthrough: Vec<String>) -> UpstreamConfig {
        UpstreamConfig {
            name: "test".to_string(),
            call_timeout_ms: 1_000,
            transport: UpstreamTransport::Stdio {
                command: "x".to_string(),
                args: vec![],
                env_passthrough,
            },
        }
    }

    #[test]
    fn build_command_applies_env_allowlist() {
        std::env::set_var("MCPGW_TEST_ALLOWED", "yes");
        std::env::set_var("MCPGW_TEST_DENIED", "secret");

        let cfg = stdio_cfg(vec!["MCPGW_TEST_ALLOWED".to_string()]);
        let cmd = build_command(&cfg);
        let std_cmd = cmd.as_std();
        let envs: Vec<_> = std_cmd.get_envs().collect();

        assert!(
            envs.iter()
                .any(|(k, v)| k.to_str() == Some("MCPGW_TEST_ALLOWED")
                    && v.map(|v| v.to_str()) == Some(Some("yes"))),
            "allow-listed var should pass through"
        );
        assert!(
            !envs
                .iter()
                .any(|(k, _)| k.to_str() == Some("MCPGW_TEST_DENIED")),
            "non-allow-listed parent var must be cleared"
        );
    }
}
