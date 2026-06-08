use serde::Deserialize;
use thiserror::Error;

/// Top-level mcpgw configuration. Only the `[retrieval]` section exists in Plan 1;
/// `[server]` and `[[upstream]]` are added in Plan 2.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub retrieval: RetrievalConfig,
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
}
