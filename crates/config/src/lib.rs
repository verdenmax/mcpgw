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
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    /// "bm25" | "vector" | "hybrid". Only "bm25" is implemented in v1.
    pub strategy: String,
    /// Number of tools `search_tools` returns.
    pub top_k: usize,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            strategy: "bm25".into(),
            top_k: 8,
        }
    }
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
        #[serde(default)]
        env_passthrough: Vec<String>,
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
}
