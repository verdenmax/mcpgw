use serde::Deserialize;
use thiserror::Error;

/// Top-level mcpgw configuration. `[retrieval]` and `[[upstream]]` exist now;
/// the `[server]` section is added in M1-B.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default, rename = "upstream")]
    pub upstreams: Vec<UpstreamConfig>,
    #[serde(default)]
    pub server: ServerConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    /// "bm25" | "vector" | "hybrid". Only "bm25" is implemented in v1.
    pub strategy: String,
    /// Number of tools `search_tools` returns.
    pub top_k: usize,
    /// `[retrieval.vector]` provider config. Required when strategy is "vector".
    pub vector: Option<VectorConfig>,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            strategy: "bm25".into(),
            top_k: 8,
            vector: None,
        }
    }
}

/// `[retrieval.vector]`: OpenAI-compatible embedding provider. Secrets via env name only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorConfig {
    #[serde(default = "default_vector_base_url")]
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    #[serde(default)]
    pub dim: Option<usize>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub batch_size: Option<usize>,
}

fn default_vector_base_url() -> String {
    "https://api.openai.com/v1".into()
}

/// `[server]` section: which downstream transport(s) to serve.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Serve the 3 meta-tools over a stdio MCP server.
    pub stdio: bool,
    /// Optional Streamable HTTP server. Omitted -> `None` (HTTP disabled).
    pub http: Option<HttpConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            stdio: true,
            http: None,
        }
    }
}

/// `[server.http]`: Streamable HTTP server settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// Start the HTTP server. Defaults to false (must opt in).
    pub enabled: bool,
    /// Bind address. Defaults to localhost; use a tunnel/reverse proxy for public exposure.
    pub bind: String,
    /// Mount path for the MCP endpoint.
    pub path: String,
    /// Accepted API keys. Empty -> no auth (relies on localhost binding).
    #[serde(rename = "api_key")]
    pub api_keys: Vec<ApiKeyConfig>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1:8970".into(),
            path: "/mcp".into(),
            api_keys: Vec::new(),
        }
    }
}

/// One accepted API key. The secret is referenced by env var name only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyConfig {
    /// Label for logs/observability. NEVER the key value.
    pub name: String,
    /// Name of the env var holding the key secret.
    pub env: String,
}

/// One upstream MCP server.
///
/// Note: this struct flattens an internally-tagged transport enum, which precludes
/// `#[serde(deny_unknown_fields)]`. Unknown keys inside an `[[upstream]]` table are
/// therefore silently ignored (e.g. a `comand` typo would be dropped, surfacing later
/// as a connection failure rather than a parse error).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UpstreamConfig {
    /// Namespace prefix for this server's tools. Must be non-blank and must not contain "__".
    pub name: String,
    /// Per-call timeout in milliseconds.
    #[serde(default = "default_call_timeout_ms")]
    pub call_timeout_ms: u64,
    #[serde(flatten)]
    pub transport: UpstreamTransport,
}

fn default_call_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum UpstreamTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// Allow-list of environment variable names passed through to the child. The child's
        /// environment is otherwise CLEARED, so only these vars (when present in mcpgw's own
        /// environment) reach the upstream process. Add e.g. "PATH"/"HOME" if the child needs them.
        #[serde(default)]
        env_passthrough: Vec<String>,
    },
    /// Remote HTTP MCP server (Streamable HTTP). Auth values referenced by env name only.
    Http {
        /// Endpoint URL, e.g. "https://example.com/mcp".
        url: String,
        /// Optional env var holding a bearer token -> sent as `Authorization: Bearer <token>`.
        #[serde(default)]
        bearer_env: Option<String>,
        /// Custom headers: header-name -> env-var-name holding the header value.
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
    },
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl Config {
    /// Parse and validate config from a TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Convenience constructor returning the all-defaults config.
    pub fn default_from_empty() -> Self {
        // Parsing an empty document applies every `#[serde(default)]`.
        Config::from_toml_str("").expect("empty config is always valid")
    }

    fn validate(&self) -> Result<(), ConfigError> {
        const KNOWN: [&str; 3] = ["bm25", "vector", "hybrid"];
        if !KNOWN.contains(&self.retrieval.strategy.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "unknown retrieval.strategy {:?} (expected one of {KNOWN:?})",
                self.retrieval.strategy
            )));
        }
        if self.retrieval.top_k == 0 {
            return Err(ConfigError::Invalid("retrieval.top_k must be > 0".into()));
        }
        if matches!(self.retrieval.strategy.as_str(), "vector" | "hybrid") {
            match &self.retrieval.vector {
                None => {
                    return Err(ConfigError::Invalid(format!(
                        "strategy={:?} requires a [retrieval.vector] section",
                        self.retrieval.strategy
                    )))
                }
                Some(v) => {
                    if v.base_url.trim().is_empty()
                        || v.model.trim().is_empty()
                        || v.api_key_env.trim().is_empty()
                    {
                        return Err(ConfigError::Invalid(
                            "[retrieval.vector] base_url/model/api_key_env must be non-empty"
                                .into(),
                        ));
                    }
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for u in &self.upstreams {
            if u.name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "upstream.name must not be empty or blank".into(),
                ));
            }
            if u.name.contains("__") {
                return Err(ConfigError::Invalid(format!(
                    "upstream.name {:?} must not contain \"__\" (namespace separator)",
                    u.name
                )));
            }
            if !seen.insert(u.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate upstream.name {:?}",
                    u.name
                )));
            }
            if let UpstreamTransport::Http { url, .. } = &u.transport {
                if url.trim().is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "upstream {:?}: http url must not be empty",
                        u.name
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(cfg.retrieval.strategy, "bm25");
        assert_eq!(cfg.retrieval.top_k, 8);
    }

    #[test]
    fn parses_retrieval_section() {
        let cfg = Config::from_toml_str(
            r#"
            [retrieval]
            strategy = "hybrid"
            top_k = 5
            [retrieval.vector]
            model = "m"
            api_key_env = "K"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.retrieval.strategy, "hybrid");
        assert_eq!(cfg.retrieval.top_k, 5);
    }

    #[test]
    fn rejects_unknown_strategy() {
        let err = Config::from_toml_str("[retrieval]\nstrategy = \"magic\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_zero_top_k() {
        let err = Config::from_toml_str("[retrieval]\ntop_k = 0\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_unknown_field_as_parse_error() {
        // `deny_unknown_fields` must turn typos / stray keys into a Parse error,
        // not silently accept them.
        let err = Config::from_toml_str("[retrieval]\nbogus = 1\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn partially_specified_section_fills_defaults() {
        // Specifying only top_k must leave strategy at its default.
        let cfg = Config::from_toml_str("[retrieval]\ntop_k = 3\n").unwrap();
        assert_eq!(cfg.retrieval.strategy, "bm25");
        assert_eq!(cfg.retrieval.top_k, 3);
    }

    #[test]
    fn parses_stdio_upstreams() {
        let cfg = Config::from_toml_str(
            r#"
            [[upstream]]
            name = "github"
            transport = "stdio"
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-github"]
            env_passthrough = ["GITHUB_TOKEN"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.upstreams.len(), 1);
        let u = &cfg.upstreams[0];
        assert_eq!(u.name, "github");
        assert_eq!(u.call_timeout_ms, 30_000); // default
        match &u.transport {
            UpstreamTransport::Stdio {
                command,
                args,
                env_passthrough,
            } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "@modelcontextprotocol/server-github"]);
                assert_eq!(env_passthrough, &["GITHUB_TOKEN"]);
            }
            UpstreamTransport::Http { .. } => unreachable!("stdio fixture"),
        }
    }

    #[test]
    fn rejects_upstream_name_with_double_underscore() {
        let err = Config::from_toml_str(
            "[[upstream]]\nname = \"a__b\"\ntransport = \"stdio\"\ncommand = \"x\"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_duplicate_upstream_names() {
        let err = Config::from_toml_str(
            "[[upstream]]\nname=\"a\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
             [[upstream]]\nname=\"a\"\ntransport=\"stdio\"\ncommand=\"y\"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn upstreams_default_to_empty() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.upstreams.is_empty());
    }

    #[test]
    fn parses_explicit_call_timeout_through_flatten() {
        // Lock the flatten path for an explicitly-specified numeric field, plus the
        // args/env_passthrough defaults when omitted.
        let cfg = Config::from_toml_str(
            "[[upstream]]\nname=\"s\"\ncall_timeout_ms=5000\ntransport=\"stdio\"\ncommand=\"x\"\n",
        )
        .unwrap();
        let u = &cfg.upstreams[0];
        assert_eq!(u.call_timeout_ms, 5000);
        match &u.transport {
            UpstreamTransport::Stdio {
                args,
                env_passthrough,
                ..
            } => {
                assert!(args.is_empty());
                assert!(env_passthrough.is_empty());
            }
            UpstreamTransport::Http { .. } => unreachable!("stdio fixture"),
        }
    }

    #[test]
    fn rejects_unknown_transport() {
        // Guards behavior once an `http` variant is added in M1-C.
        let err = Config::from_toml_str("[[upstream]]\nname=\"s\"\ntransport=\"carrier-pigeon\"\n")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn rejects_blank_upstream_name() {
        let err = Config::from_toml_str(
            "[[upstream]]\nname=\"   \"\ntransport=\"stdio\"\ncommand=\"x\"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn server_section_parses_and_defaults_to_stdio() {
        // Omitting [server] -> stdio defaults to true.
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.server.stdio);

        // Explicitly provided.
        let cfg = Config::from_toml_str("[server]\nstdio = true\n").unwrap();
        assert!(cfg.server.stdio);

        // Unknown keys rejected (ServerConfig has no flatten, so deny_unknown_fields applies).
        assert!(Config::from_toml_str("[server]\nbogus = 1\n").is_err());
    }

    #[test]
    fn parses_server_http_section_with_api_keys() {
        let cfg = Config::from_toml_str(
            r#"
            [server]
            stdio = true
            [server.http]
            enabled = true
            bind = "0.0.0.0:9000"
            path = "/gw"
            [[server.http.api_key]]
            name = "claude"
            env  = "MCPGW_KEY_CLAUDE"
            [[server.http.api_key]]
            name = "cursor"
            env  = "MCPGW_KEY_CURSOR"
            "#,
        )
        .unwrap();
        let http = cfg.server.http.expect("http section present");
        assert!(http.enabled);
        assert_eq!(http.bind, "0.0.0.0:9000");
        assert_eq!(http.path, "/gw");
        assert_eq!(http.api_keys.len(), 2);
        assert_eq!(http.api_keys[0].name, "claude");
        assert_eq!(http.api_keys[0].env, "MCPGW_KEY_CLAUDE");
    }

    #[test]
    fn server_http_defaults_when_omitted_or_partial() {
        // 整个 [server.http] 省略 -> None。
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.server.http.is_none());
        // 只给 enabled -> bind/path 用默认，api_key 空。
        let cfg = Config::from_toml_str("[server.http]\nenabled = true\n").unwrap();
        let http = cfg.server.http.unwrap();
        assert!(http.enabled);
        assert_eq!(http.bind, "127.0.0.1:8970");
        assert_eq!(http.path, "/mcp");
        assert!(http.api_keys.is_empty());
    }

    #[test]
    fn parses_http_upstream_with_bearer_and_headers() {
        let cfg = Config::from_toml_str(
            r#"
            [[upstream]]
            name = "remote"
            transport = "http"
            url = "https://example.com/mcp"
            bearer_env = "REMOTE_BEARER"
            headers = { "X-Api-Version" = "REMOTE_VER" }
            "#,
        )
        .unwrap();
        let u = &cfg.upstreams[0];
        assert_eq!(u.call_timeout_ms, 30_000); // 默认仍生效
        match &u.transport {
            UpstreamTransport::Http {
                url,
                bearer_env,
                headers,
            } => {
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(bearer_env.as_deref(), Some("REMOTE_BEARER"));
                assert_eq!(
                    headers.get("X-Api-Version").map(String::as_str),
                    Some("REMOTE_VER")
                );
            }
            _ => panic!("expected http transport"),
        }
    }

    #[test]
    fn http_upstream_url_must_not_be_blank() {
        let err =
            Config::from_toml_str("[[upstream]]\nname=\"r\"\ntransport=\"http\"\nurl=\"  \"\n")
                .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn server_http_rejects_unknown_field() {
        // HttpConfig 无 flatten -> deny_unknown_fields 生效。
        assert!(Config::from_toml_str("[server.http]\nbogus = 1\n").is_err());
    }

    #[test]
    fn parses_retrieval_vector_section() {
        let cfg = Config::from_toml_str(
            r#"
            [retrieval]
            strategy = "vector"
            [retrieval.vector]
            model = "text-embedding-3-small"
            api_key_env = "OPENAI_API_KEY"
            dim = 1536
            "#,
        )
        .unwrap();
        let v = cfg.retrieval.vector.expect("vector section");
        assert_eq!(v.base_url, "https://api.openai.com/v1"); // default
        assert_eq!(v.model, "text-embedding-3-small");
        assert_eq!(v.api_key_env, "OPENAI_API_KEY");
        assert_eq!(v.dim, Some(1536));
    }

    #[test]
    fn vector_strategy_requires_vector_section() {
        let err = Config::from_toml_str("[retrieval]\nstrategy = \"vector\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn hybrid_strategy_requires_vector_section() {
        let err = Config::from_toml_str("[retrieval]\nstrategy = \"hybrid\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn vector_section_rejects_unknown_field() {
        let err = Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"K\"\nbogus=1\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
}
