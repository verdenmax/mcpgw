//! About/Settings 只读视图的数据形状：启动时从 `config::Config` 组装的**非敏感**生效配置/限额 + 版本。
//! 隐私上 `AboutInfo` 及其嵌套类型**字段集里根本不含**任何密钥/token/env 名/env 值。

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct AboutInfo {
    pub version: VersionInfo,
    pub retrieval: RetrievalInfo,
    pub dashboard: DashboardInfo,
    pub audit: AuditInfo,
    pub server: ServerInfo,
    pub upstreams: Vec<UpstreamConfigInfo>,
}

#[derive(Serialize, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub git_sha: String,
    pub build_time: String,
}

#[derive(Serialize, Clone)]
pub struct RetrievalInfo {
    pub strategy: String,
    pub top_k: usize,
}

#[derive(Serialize, Clone)]
pub struct DashboardInfo {
    pub call_buffer: usize,
    pub payload_max_bytes: usize,
    pub trace_queries: bool,
    pub trace_buffer: usize,
    pub trace_path: Option<String>,
    /// True iff `[dashboard].admin_token_env` is configured (admin write API enabled). Never the
    /// env name or token value — a bare existence bool, mirroring `ServerInfo.http_auth`.
    pub admin_enabled: bool,
}

#[derive(Serialize, Clone)]
pub struct AuditInfo {
    pub enabled: bool,
    pub path: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct ServerInfo {
    pub stdio: bool,
    pub http_enabled: bool,
    pub http_bind: Option<String>,
    pub http_path: Option<String>,
    pub http_auth: bool,
}

#[derive(Serialize, Clone)]
pub struct UpstreamConfigInfo {
    pub name: String,
    pub transport: String,
    pub call_timeout_ms: u64,
}

/// 上游 transport → 短标签（自包含，避免 dashboard 反向依赖 mcpgw 的 `transport_str`）。
fn transport_label(t: &config::UpstreamTransport) -> &'static str {
    match t {
        config::UpstreamTransport::Stdio { .. } => "stdio",
        config::UpstreamTransport::Http { .. } => "http",
    }
}

impl AboutInfo {
    /// 从生效配置 + 版本组装只读 About 视图。仅非敏感字段：绝不含密钥/token/env 名/值/上游认证引用。
    pub fn from_config(cfg: &config::Config, version: VersionInfo) -> AboutInfo {
        let (http_enabled, http_bind, http_path, http_auth) = match &cfg.server.http {
            Some(h) if h.enabled => (
                true,
                Some(h.bind.clone()),
                Some(h.path.clone()),
                !h.api_keys.is_empty(),
            ),
            _ => (false, None, None, false),
        };
        AboutInfo {
            version,
            retrieval: RetrievalInfo {
                strategy: cfg.retrieval.strategy.clone(),
                top_k: cfg.retrieval.top_k,
            },
            dashboard: DashboardInfo {
                call_buffer: cfg.dashboard.call_buffer,
                payload_max_bytes: cfg.dashboard.payload_max_bytes,
                trace_queries: cfg.dashboard.trace_queries,
                trace_buffer: cfg.dashboard.trace_buffer,
                trace_path: cfg.dashboard.trace_path.clone(),
                admin_enabled: cfg.dashboard.admin_token_env.is_some(),
            },
            audit: AuditInfo {
                enabled: cfg.audit.enabled,
                path: cfg.audit.enabled.then(|| cfg.audit.path.clone()),
            },
            server: ServerInfo {
                stdio: cfg.server.stdio,
                http_enabled,
                http_bind,
                http_path,
                http_auth,
            },
            upstreams: cfg
                .upstreams
                .iter()
                .map(|u| UpstreamConfigInfo {
                    name: u.name.clone(),
                    transport: transport_label(&u.transport).to_string(),
                    call_timeout_ms: u.call_timeout_ms,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver() -> VersionInfo {
        VersionInfo {
            version: "0.1.0".into(),
            git_sha: "abc123".into(),
            build_time: "0".into(),
        }
    }

    #[test]
    fn from_config_maps_non_sensitive_fields() {
        let toml = "[retrieval]\nstrategy = \"bm25\"\ntop_k = 7\n\
                    [dashboard]\nenabled = true\ncall_buffer = 1234\npayload_max_bytes = 4096\n\
                    [[upstream]]\nname = \"mock\"\ntransport = \"stdio\"\ncommand = \"x\"\n";
        let cfg = config::Config::from_toml_str(toml).unwrap();
        let a = AboutInfo::from_config(&cfg, ver());
        assert_eq!(a.retrieval.strategy, "bm25");
        assert_eq!(a.retrieval.top_k, 7);
        assert_eq!(a.dashboard.call_buffer, 1234);
        assert_eq!(a.dashboard.payload_max_bytes, 4096);
        assert!(!a.audit.enabled);
        assert_eq!(a.audit.path, None);
        assert!(!a.server.http_enabled);
        assert_eq!(a.upstreams.len(), 1);
        assert_eq!(a.upstreams[0].name, "mock");
        assert_eq!(a.upstreams[0].transport, "stdio");
        assert!(!a.dashboard.admin_enabled, "no admin_token_env -> false");
        assert_eq!(a.version.version, "0.1.0");
    }

    #[test]
    fn http_auth_true_and_no_secrets_leak() {
        let toml = "[retrieval]\nstrategy = \"bm25\"\n\
                    [server.http]\nenabled = true\nbind = \"0.0.0.0:9000\"\npath = \"/mcp\"\n\
                    [[server.http.api_key]]\nname = \"keylabel\"\nenv = \"SECRET_KEY\"\n\
                    [[upstream]]\nname = \"remote\"\ntransport = \"http\"\nurl = \"https://example.com/mcp\"\nbearer_env = \"REMOTE_TOKEN\"\ncall_timeout_ms = 5000\n";
        let cfg = config::Config::from_toml_str(toml).unwrap();
        let a = AboutInfo::from_config(&cfg, ver());
        assert!(a.server.http_enabled);
        assert!(a.server.http_auth, "api_key present -> auth enabled");
        assert_eq!(a.upstreams[0].transport, "http");
        assert_eq!(a.upstreams[0].call_timeout_ms, 5000);
        let json = serde_json::to_string(&a).unwrap();
        for secret in [
            "SECRET_KEY",
            "REMOTE_TOKEN",
            "keylabel",
            "example.com",
            "bearer_env",
            "api_key",
        ] {
            assert!(
                !json.contains(secret),
                "About JSON must not leak {secret:?}: {json}"
            );
        }
    }

    #[test]
    fn admin_enabled_reflects_config_without_leaking_env_name() {
        let toml = "[retrieval]\nstrategy = \"bm25\"\n\
                    [dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_DASH_ADMIN\"\n";
        let cfg = config::Config::from_toml_str(toml).unwrap();
        let a = AboutInfo::from_config(&cfg, ver());
        assert!(a.dashboard.admin_enabled, "admin_token_env set -> true");
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            !json.contains("MCPGW_DASH_ADMIN"),
            "About JSON must not leak the admin env var name: {json}"
        );
    }
}
