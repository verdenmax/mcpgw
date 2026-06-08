use catalog::Catalog;

/// A retrieval hit: a tool's qualified name, its description, and a relevance score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredTool {
    pub qualified_name: String,
    pub description: String,
    pub score: f32,
}

/// A pluggable tool-retrieval strategy (BM25, vector, hybrid, ...).
pub trait RetrievalStrategy: Send + Sync {
    /// (Re)build internal indices from the current catalog.
    fn index(&mut self, catalog: &Catalog);
    /// Return up to `top_k` tools relevant to `query`, best first.
    fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}

/// Lowercase, split on any non-alphanumeric boundary (this also splits `_`), drop empties.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_on_non_alphanumeric_and_lowercases() {
        assert_eq!(
            tokenize("GitHub__create_issue"),
            vec!["github", "create", "issue"]
        );
        assert_eq!(tokenize("  multiple,, spaces "), vec!["multiple", "spaces"]);
        assert!(tokenize("").is_empty());
    }
}
