//! `SubagentStrategy`: BM25 prefilter -> small chat model reranks the shortlist (retrieve-then-
//! rerank). Falls back transparently to the BM25 shortlist when the chat call or its parse
//! fails. Prompt construction and response parsing live here (pure, MockChatModel-testable);
//! the HTTP chat client lives in the separate `chat` crate.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use catalog::Catalog;

use crate::{Bm25Strategy, ChatModel, RetrievalStrategy, ScoredTool};

/// Default prefilter shortlist size handed to the chat model.
pub const DEFAULT_CANDIDATES: usize = 20;

const SYSTEM_PROMPT: &str = "You are a tool selector. Given a user query and a numbered list of \
candidate tools (qualified_name: description), choose the tools most relevant to the query. \
Reply with ONLY a JSON array of the chosen qualified_names, most relevant first, no more than \
the number requested, choosing ONLY from the candidates. No prose, no code fences.";

/// Build the user prompt: the query, the requested count, and the numbered candidate list.
fn build_user_prompt(query: &str, shortlist: &[ScoredTool], top_k: usize) -> String {
    let mut s = format!("Query: {query}\nReturn at most {top_k} qualified_names.\nCandidates:\n");
    for (i, t) in shortlist.iter().enumerate() {
        s.push_str(&format!(
            "{}. {}: {}\n",
            i + 1,
            t.qualified_name,
            t.description
        ));
    }
    s
}

/// Parse the model reply into ordered qualified_names, keeping only names present in `allowed`
/// (drops hallucinations), de-duplicated, order-preserving. Empty on any failure (caller then
/// degrades to BM25).
fn parse_selection(reply: &str, allowed: &[ScoredTool]) -> Vec<String> {
    // Extract the span from the first `[` to the last `]` (models sometimes wrap the array in
    // prose / code fences). Greedy by design; non-array spans then fail JSON parsing and degrade.
    let arr = match (reply.find('['), reply.rfind(']')) {
        (Some(a), Some(b)) if b > a => &reply[a..=b],
        _ => return Vec::new(),
    };
    let names: Vec<String> = match serde_json::from_str::<Vec<String>>(arr) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let allowed_set: HashSet<&str> = allowed.iter().map(|t| t.qualified_name.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    names
        .into_iter()
        .filter(|n| allowed_set.contains(n.as_str()) && seen.insert(n.clone()))
        .collect()
}

/// BM25 prefilter + chat-model rerank, with transparent BM25 fallback. Construct via
/// `build_strategy("subagent", &Backends { chat: Some(..), .. })` or directly with `new`.
pub struct SubagentStrategy {
    bm25: Bm25Strategy,
    chat: Arc<dyn ChatModel>,
    candidates: usize,
}

impl SubagentStrategy {
    pub fn new(chat: Arc<dyn ChatModel>, candidates: usize) -> Self {
        Self {
            bm25: Bm25Strategy::new(),
            chat,
            candidates: candidates.max(1),
        }
    }
}

#[async_trait]
impl RetrievalStrategy for SubagentStrategy {
    async fn index(&mut self, catalog: &Catalog) {
        self.bm25.index(catalog).await;
    }

    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        // BM25 prefilter -> candidate shortlist.
        let shortlist = self.bm25.search(query, self.candidates).await;
        if shortlist.is_empty() {
            return Vec::new(); // no lexical match -> nothing to rerank
        }

        let names = match self
            .chat
            .complete(SYSTEM_PROMPT, &build_user_prompt(query, &shortlist, top_k))
            .await
        {
            Ok(reply) => {
                let sel = parse_selection(&reply, &shortlist);
                if sel.is_empty() {
                    tracing::debug!(
                        "subagent reply yielded no valid tools; falling back to BM25 shortlist"
                    );
                }
                sel
            }
            Err(e) => {
                tracing::warn!(error = %e, "subagent chat failed; falling back to BM25 shortlist");
                Vec::new()
            }
        };

        if names.is_empty() {
            // Degrade: return the BM25 shortlist (already ranked), truncated.
            let mut out = shortlist;
            out.truncate(top_k);
            return out;
        }

        // Map chosen names back to ScoredTool with a synthetic descending score (order only).
        let by_name: HashMap<&str, &ScoredTool> = shortlist
            .iter()
            .map(|t| (t.qualified_name.as_str(), t))
            .collect();
        let n = names.len();
        names
            .iter()
            .take(top_k)
            .enumerate()
            .filter_map(|(i, name)| {
                by_name.get(name.as_str()).map(|t| ScoredTool {
                    qualified_name: t.qualified_name.clone(),
                    description: t.description.clone(),
                    score: (n - i) as f32,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ScoredTool {
        ScoredTool {
            qualified_name: name.into(),
            description: String::new(),
            score: 0.0,
        }
    }

    #[test]
    fn parse_keeps_only_allowed_in_order_dedup() {
        let allowed = vec![tool("a__x"), tool("b__y"), tool("c__z")];
        // contains prose around the array, a hallucination ("zzz"), and a duplicate ("a__x").
        let reply = r#"sure: ["b__y", "zzz", "a__x", "a__x"] -- done"#;
        assert_eq!(
            parse_selection(reply, &allowed),
            vec!["b__y".to_string(), "a__x".to_string()]
        );
    }

    #[test]
    fn parse_returns_empty_on_garbage_or_empty_array() {
        let allowed = vec![tool("a__x")];
        assert!(parse_selection("no json here", &allowed).is_empty());
        assert!(parse_selection("[not valid json", &allowed).is_empty());
        assert!(parse_selection("[]", &allowed).is_empty());
    }

    #[test]
    fn parse_returns_empty_on_non_string_json_array() {
        // Valid JSON array but not of strings -> `from_str::<Vec<String>>` errors -> empty
        // (exercises the serde parse-error arm, not just the bracket-extraction guard).
        let allowed = vec![tool("a__x")];
        assert!(parse_selection("[1, 2, 3]", &allowed).is_empty());
    }
}
