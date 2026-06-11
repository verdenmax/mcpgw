//! Eager, degraded-start connection of all configured upstreams (real stdio children).

use std::sync::Arc;
use std::time::Duration;

use config::{UpstreamConfig, UpstreamTransport};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
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
pub(crate) fn build_command(
    command: &str,
    args: &[String],
    env_passthrough: &[String],
) -> tokio::process::Command {
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
    let UpstreamTransport::Stdio {
        command,
        args,
        env_passthrough,
    } = &cfg.transport
    else {
        return Err(UpstreamError::Connect {
            server: cfg.name.clone(),
            source: "connect_stdio_upstream called on a non-stdio upstream".into(),
        });
    };
    let cmd = build_command(command, args, env_passthrough);
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

/// Resolve env-referenced auth into an rmcp client transport config. No I/O beyond env reads.
/// A referenced-but-missing env var is a hard error (auth must not be silently dropped).
fn build_http_client_config(
    name: &str,
    url: &str,
    bearer_env: &Option<String>,
    headers: &std::collections::HashMap<String, String>,
) -> Result<StreamableHttpClientTransportConfig, UpstreamError> {
    let mut cfg = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    if let Some(env_name) = bearer_env {
        let token = std::env::var(env_name).map_err(|_| UpstreamError::Connect {
            server: name.to_string(),
            source: format!("missing env {env_name:?} for bearer_env").into(),
        })?;
        cfg = cfg.auth_header(format!("Bearer {token}"));
    }
    if !headers.is_empty() {
        let mut custom = std::collections::HashMap::new();
        for (hname, env_name) in headers {
            let val = std::env::var(env_name).map_err(|_| UpstreamError::Connect {
                server: name.to_string(),
                source: format!("missing env {env_name:?} for header {hname:?}").into(),
            })?;
            let hn = http::HeaderName::from_bytes(hname.as_bytes()).map_err(|e| {
                UpstreamError::Connect {
                    server: name.to_string(),
                    source: Box::new(e),
                }
            })?;
            let hv = http::HeaderValue::from_str(&val).map_err(|e| UpstreamError::Connect {
                server: name.to_string(),
                source: Box::new(e),
            })?;
            custom.insert(hn, hv);
        }
        cfg = cfg.custom_headers(custom);
    }
    Ok(cfg)
}

/// Connect one HTTP upstream. Handshake bounded by `call_timeout_ms`, same as stdio.
pub async fn connect_http_upstream(
    cfg: &UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError> {
    let UpstreamTransport::Http {
        url,
        bearer_env,
        headers,
    } = &cfg.transport
    else {
        return Err(UpstreamError::Connect {
            server: cfg.name.clone(),
            source: "connect_http_upstream called on a non-http upstream".into(),
        });
    };
    let client_cfg = build_http_client_config(&cfg.name, url, bearer_env, headers)?;
    let transport = StreamableHttpClientTransport::from_config(client_cfg);
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
        let result = match &cfg.transport {
            UpstreamTransport::Stdio { .. } => {
                connect_stdio_upstream(cfg, Some(trigger.clone())).await
            }
            UpstreamTransport::Http { .. } => {
                connect_http_upstream(cfg, Some(trigger.clone())).await
            }
        };
        match result {
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
        let UpstreamTransport::Stdio {
            command,
            args,
            env_passthrough,
        } = &cfg.transport
        else {
            unreachable!()
        };
        let cmd = build_command(command, args, env_passthrough);
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

    fn http_cfg(bearer_env: Option<&str>, headers: &[(&str, &str)]) -> UpstreamConfig {
        UpstreamConfig {
            name: "remote".to_string(),
            call_timeout_ms: 1_000,
            transport: UpstreamTransport::Http {
                url: "http://127.0.0.1:1/mcp".to_string(),
                bearer_env: bearer_env.map(str::to_string),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            },
        }
    }

    #[test]
    fn build_http_client_config_sets_bearer_and_headers_from_env() {
        std::env::set_var("MCPGW_T2_BEARER", "sekret");
        std::env::set_var("MCPGW_T2_VER", "2024-01");
        let cfg = http_cfg(
            Some("MCPGW_T2_BEARER"),
            &[("X-Api-Version", "MCPGW_T2_VER")],
        );
        let UpstreamTransport::Http {
            url,
            bearer_env,
            headers,
        } = &cfg.transport
        else {
            unreachable!()
        };
        let client_cfg = build_http_client_config(&cfg.name, url, bearer_env, headers).unwrap();
        assert_eq!(client_cfg.auth_header.as_deref(), Some("Bearer sekret"));
        assert_eq!(
            client_cfg
                .custom_headers
                .get(&http::HeaderName::from_static("x-api-version"))
                .map(|v| v.to_str().unwrap()),
            Some("2024-01")
        );
    }

    #[test]
    fn build_http_client_config_missing_env_is_error() {
        let cfg = http_cfg(Some("MCPGW_T2_DEFINITELY_MISSING"), &[]);
        let UpstreamTransport::Http {
            url,
            bearer_env,
            headers,
        } = &cfg.transport
        else {
            unreachable!()
        };
        let err = build_http_client_config(&cfg.name, url, bearer_env, headers).unwrap_err();
        assert!(matches!(err, UpstreamError::Connect { .. }));
    }

    #[test]
    fn build_http_client_config_no_auth_is_ok() {
        let cfg = http_cfg(None, &[]);
        let UpstreamTransport::Http {
            url,
            bearer_env,
            headers,
        } = &cfg.transport
        else {
            unreachable!()
        };
        let client_cfg = build_http_client_config(&cfg.name, url, bearer_env, headers).unwrap();
        assert!(client_cfg.auth_header.is_none());
        assert!(client_cfg.custom_headers.is_empty());
    }
}
