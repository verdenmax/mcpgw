# M2-B：混合检索（RRF 融合 BM25 + 向量）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 `"hybrid"` 检索策略，用 Reciprocal Rank Fusion（RRF, k=60）融合 `Bm25Strategy`（词法）与
`VectorStrategy`（语义）的排名；opt-in（默认仍 `bm25`），并打通 config 校验与 `mcpgw` 启动期接线。

**Architecture:** 新增 `retrieval::HybridStrategy`，内部组合现有 `Bm25Strategy` + `VectorStrategy`；
`search` 取两份**全深度**子排名后做 RRF 融合。`build_strategy("hybrid", Some(embedder))` 构造之；
`config::validate` 把「需要 `[retrieval.vector]`」从 `vector` 扩到 `hybrid`；`mcpgw::build_embedder` 对
`hybrid` 也建 embedder。`metatools`/`gateway`/`downstream` 无需改动（已对策略/embedder/async 泛化）。

**Tech Stack:** Rust 2021 / `async-trait` / tokio（dev）/ 复用 M2-A 的 `MockEmbedder`（testkit）。

**Spec:** `docs/superpowers/specs/2026-06-13-mcpgw-m2b-hybrid-retrieval-design.md`

---

## 已确认的关键事实（实现时照用）

- `RetrievalStrategy` 是 **async** trait（`async fn index(&mut self, &Catalog)` / `async fn search(&self, &str, usize) -> Vec<ScoredTool>`）。
- `Bm25Strategy::search` 带 `score > 0.0` 过滤，**只返回命中词项的文档**；`VectorStrategy::search` 对**全部**文档余弦排名（含 cosine 0）。
- `VectorStrategy::search` 在 `degraded`/无向量/本次 query 嵌入失败时返回**内部 BM25** 结果（透明降级）。
- 两个策略的排序均为「score 降序 + `qualified_name` 升序」tie-break（确定性，golden 依赖）。
- `MockEmbedder`（`testkit` feature）：`MockEmbedder::new(dim)`（确定性、token 分桶余弦）与 `MockEmbedder::failing(dim)`（`embed` 永远 `Err`，驱动降级测试）。
- `retrieval` 的 testkit 集成测试放在 `crates/retrieval/tests/<name>.rs`，并在 `Cargo.toml` 用 `[[test]] name=... required-features=["testkit"]` 门控（见 `vector`/`caching`/`embedder`）。
- `build_embedder` 的文档注释已写 “vector/hybrid”，但 `match` 臂目前只处理 `"vector"`；`build_embedder` 的成功测试用真实 `OpenAiEmbedder`（env key），**不**用 MockEmbedder。
- `gateway` 的 dev-dep 已含 `retrieval`/`upstream` 的 `testkit`，且 `metatools` 为普通依赖（现有 vector 快照测试已用 `retrieval::MockEmbedder` + `metatools::search_tools`）。
- `config::validate` 的 `KNOWN` 白名单已含 `"hybrid"`；当前仅 `strategy=="vector"` 才要求 `[retrieval.vector]`。
- **已知文档漂移**：`docs/L4-api/retrieval-lib.md` 仍写同步 trait、旧 `build_strategy(name)` 签名、缺 `EmbedderRequired` 变体（M2-A 漏更）。本计划 Task 7 顺带订正（因正改 `build_strategy`/`StrategyError`）。

## File Structure

| 文件 | 职责 | 任务 |
|------|------|------|
| `crates/retrieval/src/hybrid.rs`（新） | `HybridStrategy` + 私有 `rrf_fuse`；纯 RRF 单测 | T1 |
| `crates/retrieval/src/lib.rs` | `mod hybrid;` + `pub use HybridStrategy`；`build_strategy` 加 `"hybrid"` 臂；更新错误测试 | T1/T2 |
| `crates/retrieval/tests/hybrid.rs`（新） | testkit 集成测试（index/search/降级/空/不对称/工厂） | T3 |
| `crates/retrieval/Cargo.toml` | 加 `[[test]] name="hybrid" required-features=["testkit"]` | T3 |
| `crates/config/src/lib.rs` | `validate`：`vector` → `vector\|hybrid` 都要求 `[retrieval.vector]`；更新/新增测试 | T4 |
| `crates/mcpgw/src/main.rs` | `build_embedder` 匹配臂 `"vector"` → `"vector"\|"hybrid"`；加测试 | T5 |
| `crates/gateway/src/lib.rs` | 加 `with_embedder("hybrid", ...)` 快照测试（镜像 vector 用例） | T6 |
| `docs/L1`–`L4` / `README` / roadmap | 分层文档（DoD，同提交） | T7 |

## 前置：建分支

实现在分支 `feat/m2b-hybrid-retrieval` 上进行（spec/plan 已提交 master）。第一个 task 开始前：

```bash
git switch -c feat/m2b-hybrid-retrieval
```

---

### Task 1: `HybridStrategy` + `rrf_fuse`（核心 + 纯 RRF 单测）

**Files:**
- Create: `crates/retrieval/src/hybrid.rs`
- Modify: `crates/retrieval/src/lib.rs`（加 `mod hybrid;` 与 `pub use hybrid::HybridStrategy;`）

- [ ] **Step 1: 写文件（含 `rrf_fuse` 桩 + 结构/impl + 失败单测）**

创建 `crates/retrieval/src/hybrid.rs`，`rrf_fuse` 先返回空 `Vec`（桩，使其编译但单测失败）：

```rust
//! `HybridStrategy`: Reciprocal Rank Fusion (RRF) over a BM25 ranking and a vector ranking.
//!
//! Reuses `Bm25Strategy` (lexical) and `VectorStrategy` (semantic; itself self-degrades to
//! BM25 when the embedder is unavailable). RRF fuses by *rank*, so the two differently-scaled
//! score lists combine without normalization, and degradation self-heals: when the vector list
//! collapses to BM25 ranks, the fused order tracks BM25.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use catalog::Catalog;

use crate::embedder::Embedder;
use crate::{Bm25Strategy, RetrievalStrategy, ScoredTool, VectorStrategy};

/// Industry-standard RRF damping constant. Larger `k` flattens the rank weighting.
const RRF_K: f32 = 60.0;

/// Fuse ranked lists by Reciprocal Rank Fusion: each list contributes `1 / (RRF_K + rank)`
/// (rank from 1) to a document's score, summed by qualified name. Deterministic: ties break on
/// `qualified_name` ascending. Truncates to `top_k`.
fn rrf_fuse(lists: &[Vec<ScoredTool>], top_k: usize) -> Vec<ScoredTool> {
    // STUB — replaced in Step 3.
    let _ = (lists, top_k);
    Vec::new()
}

/// RRF hybrid of BM25 + vector retrieval. Requires an `Embedder` (the vector arm); construct via
/// `build_strategy("hybrid", Some(embedder))` or directly with `new`.
pub struct HybridStrategy {
    bm25: Bm25Strategy,
    vector: VectorStrategy,
    doc_count: usize,
}

impl HybridStrategy {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            bm25: Bm25Strategy::new(),
            vector: VectorStrategy::new(embedder),
            doc_count: 0,
        }
    }
}

#[async_trait]
impl RetrievalStrategy for HybridStrategy {
    async fn index(&mut self, catalog: &Catalog) {
        self.bm25.index(catalog).await;
        self.vector.index(catalog).await;
        self.doc_count = catalog.iter().count();
    }

    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        if self.doc_count == 0 {
            return Vec::new();
        }
        // Full-depth sub-rankings: RRF needs each doc's true rank in each list, so we must not
        // pre-truncate to top_k (that would drop one-sided matches before fusion).
        let lb = self.bm25.search(query, self.doc_count).await;
        let lv = self.vector.search(query, self.doc_count).await;
        rrf_fuse(&[lb, lv], top_k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, score: f32) -> ScoredTool {
        ScoredTool {
            qualified_name: name.into(),
            description: format!("desc of {name}"),
            score,
        }
    }

    #[test]
    fn rrf_ranks_doc_high_in_both_lists_first() {
        // a: rank1 in both -> 2/61 ; b: rank2 in both -> 2/62. a first.
        let l1 = vec![hit("a", 9.0), hit("b", 8.0)];
        let l2 = vec![hit("a", 0.9), hit("b", 0.8)];
        let out = rrf_fuse(&[l1, l2], 10);
        assert_eq!(
            out.iter().map(|h| h.qualified_name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!((out[0].score - 2.0 / 61.0).abs() < 1e-6);
        assert!((out[1].score - 2.0 / 62.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_breaks_ties_on_qualified_name() {
        // "b" rank1 in list1 only; "a" rank1 in list2 only -> equal 1/61 -> name asc.
        let out = rrf_fuse(&[vec![hit("b", 1.0)], vec![hit("a", 1.0)]], 10);
        assert_eq!(
            out.iter().map(|h| h.qualified_name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!((out[0].score - 1.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_includes_doc_present_in_only_one_list() {
        // a: 1/61 + 1/61 ; c: 1/62 -> a first, c present.
        let out = rrf_fuse(&[vec![hit("a", 1.0)], vec![hit("a", 1.0), hit("c", 0.5)]], 10);
        assert_eq!(
            out.iter().map(|h| h.qualified_name.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn rrf_respects_top_k() {
        let out = rrf_fuse(&[vec![hit("a", 3.0), hit("b", 2.0), hit("c", 1.0)]], 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].qualified_name, "a");
        assert_eq!(out[1].qualified_name, "b");
    }

    #[test]
    fn rrf_empty_lists_yield_empty() {
        assert!(rrf_fuse(&[], 5).is_empty());
        assert!(rrf_fuse(&[Vec::new(), Vec::new()], 5).is_empty());
    }
}
```

在 `crates/retrieval/src/lib.rs` 顶部模块区把：

```rust
mod caching;
mod embedder;
mod vector;
pub use caching::CachingEmbedder;
```

改为（新增 `mod hybrid;` 与 `pub use`）：

```rust
mod caching;
mod embedder;
mod hybrid;
mod vector;
pub use caching::CachingEmbedder;
pub use hybrid::HybridStrategy;
```

- [ ] **Step 2: 运行单测，确认失败**

Run: `cargo test -p retrieval hybrid::tests -- --nocapture`
Expected: 5 个 `rrf_*` 测试 FAIL（桩返回空 → 断言不满足，如 `rrf_ranks_doc_high_in_both_lists_first` 在 `out[0]` 索引越界 panic 或长度断言失败）。

- [ ] **Step 3: 实现 `rrf_fuse`**

把 `hybrid.rs` 的 `rrf_fuse` 桩替换为真实实现：

```rust
fn rrf_fuse(lists: &[Vec<ScoredTool>], top_k: usize) -> Vec<ScoredTool> {
    // qualified_name -> (fused_score, description)
    let mut fused: HashMap<String, (f32, String)> = HashMap::new();
    for list in lists {
        for (i, hit) in list.iter().enumerate() {
            let rank = (i + 1) as f32;
            let contrib = 1.0 / (RRF_K + rank);
            let entry = fused
                .entry(hit.qualified_name.clone())
                .or_insert_with(|| (0.0, hit.description.clone()));
            entry.0 += contrib;
        }
    }
    let mut scored: Vec<ScoredTool> = fused
        .into_iter()
        .map(|(qualified_name, (score, description))| ScoredTool {
            qualified_name,
            description,
            score,
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.qualified_name.cmp(&b.qualified_name))
    });
    scored.truncate(top_k);
    scored
}
```

- [ ] **Step 4: 运行单测，确认通过**

Run: `cargo test -p retrieval hybrid::tests`
Expected: 5 个 `rrf_*` 测试全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/retrieval/src/hybrid.rs crates/retrieval/src/lib.rs
git commit -m "feat(retrieval): HybridStrategy + RRF fusion core (M2-B T1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 2: `build_strategy` 的 `"hybrid"` 臂

**Files:**
- Modify: `crates/retrieval/src/lib.rs`（`build_strategy` match + `build_strategy_errors_appropriately` 测试）

- [ ] **Step 1: 更新错误测试（先失败）**

把 `crates/retrieval/src/lib.rs` 中现有的 `build_strategy_errors_appropriately` 测试整体替换为：

```rust
    #[test]
    fn build_strategy_errors_appropriately() {
        // hybrid without an embedder now errors as EmbedderRequired (was NotImplemented pre-M2-B).
        assert!(matches!(
            build_strategy("hybrid", None),
            Err(StrategyError::EmbedderRequired(_))
        ));
        assert!(matches!(
            build_strategy("vector", None),
            Err(StrategyError::EmbedderRequired(_))
        ));
        // A genuinely unknown name is still NotImplemented.
        assert!(matches!(
            build_strategy("nope", None),
            Err(StrategyError::NotImplemented(_))
        ));
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p retrieval build_strategy_errors_appropriately`
Expected: FAIL —— 当前 `"hybrid"` 走 `NotImplemented` 臂，第一个 `matches!(EmbedderRequired)` 断言不满足。

- [ ] **Step 3: 加 `"hybrid"` 臂**

在 `crates/retrieval/src/lib.rs` 的 `build_strategy` 中，`"vector"` 臂之后、`other` 臂之前插入：

```rust
        "hybrid" => match embedder {
            Some(e) => Ok(Box::new(HybridStrategy::new(e.clone()))),
            None => Err(StrategyError::EmbedderRequired(name.to_string())),
        },
```

（`HybridStrategy` 已由 Task 1 的 `pub use hybrid::HybridStrategy;` 引入作用域。）

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p retrieval`
Expected: PASS（含 `build_strategy_errors_appropriately` 与 Task 1 的 `rrf_*`）。

- [ ] **Step 5: 提交**

```bash
git add crates/retrieval/src/lib.rs
git commit -m "feat(retrieval): build_strategy 'hybrid' arm requires embedder (M2-B T2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 3: hybrid 集成测试（testkit，经 `MockEmbedder`）

**Files:**
- Create: `crates/retrieval/tests/hybrid.rs`
- Modify: `crates/retrieval/Cargo.toml`（加 `[[test]]`）

- [ ] **Step 1: 加 Cargo 测试目标**

在 `crates/retrieval/Cargo.toml` 末尾（其它 `[[test]]` 之后）追加：

```toml
[[test]]
name = "hybrid"
required-features = ["testkit"]
```

- [ ] **Step 2: 写集成测试**

创建 `crates/retrieval/tests/hybrid.rs`：

```rust
//! Hybrid (RRF) integration tests over the deterministic MockEmbedder.
use std::sync::Arc;

use catalog::{Catalog, ToolDef};
use retrieval::{build_strategy, Bm25Strategy, Embedder, HybridStrategy, MockEmbedder, RetrievalStrategy};
use serde_json::Value;

fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
    ToolDef {
        server: server.into(),
        name: name.into(),
        description: desc.into(),
        input_schema: Value::Null,
    }
}

fn sample() -> Catalog {
    Catalog::from_tooldefs(vec![
        tool("github", "create_issue", "Create a new issue in a GitHub repository"),
        tool("github", "list_pull_requests", "List pull requests for a repository"),
        tool("slack", "post_message", "Send a chat message to a Slack channel"),
        tool("weather", "get_forecast", "Get the weather forecast for a location"),
    ])
}

#[tokio::test]
async fn hybrid_ranks_relevant_tool_first() {
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::new(64)));
    h.index(&sample()).await;
    let hits = h.search("create github issue", 3).await;
    assert!(!hits.is_empty());
    assert_eq!(hits[0].qualified_name, "github__create_issue");
    for w in hits.windows(2) {
        assert!(w[0].score >= w[1].score, "scores must be descending");
    }
}

#[tokio::test]
async fn hybrid_degrades_to_bm25_order_when_embedder_fails() {
    // failing embedder -> VectorStrategy.search returns its internal BM25 list, so both fused
    // lists are identical BM25 rankings -> hybrid order matches standalone BM25.
    let cat = sample();
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::failing(64)));
    h.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    let hq: Vec<String> = h.search("repository", 10).await.into_iter().map(|x| x.qualified_name).collect();
    let bq: Vec<String> = b.search("repository", 10).await.into_iter().map(|x| x.qualified_name).collect();
    assert_eq!(hq, bq);
    assert!(!hq.is_empty(), "query 'repository' matches at least one tool");
}

#[tokio::test]
async fn hybrid_empty_catalog_returns_empty() {
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::new(64)));
    h.index(&Catalog::new()).await;
    assert!(h.search("anything", 5).await.is_empty());
}

#[tokio::test]
async fn hybrid_surfaces_vector_candidates_when_bm25_empty() {
    // No lexical overlap: BM25 alone returns nothing, but the vector list ranks all docs, so
    // hybrid still returns candidates (semantic recall).
    let cat = sample();
    let mut h = HybridStrategy::new(Arc::new(MockEmbedder::new(64)));
    h.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    assert!(b.search("zzzznonexistent", 5).await.is_empty());
    assert!(!h.search("zzzznonexistent", 5).await.is_empty());
}

#[tokio::test]
async fn build_strategy_hybrid_with_embedder_indexes_and_searches() {
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(64));
    let mut strat = build_strategy("hybrid", Some(&embedder)).expect("hybrid ok with embedder");
    strat.index(&sample()).await;
    let hits = strat.search("forecast", 8).await;
    assert_eq!(
        hits.first().map(|h| h.qualified_name.as_str()),
        Some("weather__get_forecast")
    );
}
```

- [ ] **Step 3: 运行，确认通过**

Run: `cargo test -p retrieval --features testkit --test hybrid`
Expected: 5 个测试全部 PASS。

- [ ] **Step 4: 提交**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/tests/hybrid.rs
git commit -m "test(retrieval): hybrid integration tests via MockEmbedder (M2-B T3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: config 校验 —— `hybrid` 也要求 `[retrieval.vector]`

**Files:**
- Modify: `crates/config/src/lib.rs`（`validate` 块 + `parses_retrieval_section` 测试 + 新增测试）

- [ ] **Step 1: 更新/新增测试（先失败）**

(a) 把现有 `parses_retrieval_section` 测试整体替换为（补 `[retrieval.vector]` 使 `hybrid` 在新规则下仍合法）：

```rust
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
```

(b) 在 `crates/config/src/lib.rs` 的 `tests` 模块新增（紧跟 `vector_strategy_requires_vector_section` 之后）：

```rust
    #[test]
    fn hybrid_strategy_requires_vector_section() {
        let err = Config::from_toml_str("[retrieval]\nstrategy = \"hybrid\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p config hybrid_strategy_requires_vector_section`
Expected: FAIL —— 当前 `validate` 不对 `hybrid` 要求 vector 段，故无 `Invalid`（测试断言失败）。

- [ ] **Step 3: 放宽 `validate`**

把 `crates/config/src/lib.rs` `validate()` 中的整段：

```rust
        if self.retrieval.strategy == "vector" {
            match &self.retrieval.vector {
                None => {
                    return Err(ConfigError::Invalid(
                        "strategy=\"vector\" requires a [retrieval.vector] section".into(),
                    ))
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
```

替换为（条件扩到 `vector|hybrid`，缺段消息含策略名）：

```rust
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
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p config`
Expected: PASS（含更新后的 `parses_retrieval_section`、新增的 `hybrid_strategy_requires_vector_section`，以及既有 `vector_strategy_requires_vector_section`）。

- [ ] **Step 5: 提交**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): hybrid strategy also requires [retrieval.vector] (M2-B T4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: `mcpgw::build_embedder` 对 `hybrid` 也建 embedder

**Files:**
- Modify: `crates/mcpgw/src/main.rs`（`build_embedder` match 臂 + 错误消息 + 新增测试）

- [ ] **Step 1: 新增测试（先失败）**

在 `crates/mcpgw/src/main.rs` 的 `tests` 模块，紧跟 `build_embedder_some_for_vector_with_key` 之后新增：

```rust
    #[test]
    fn build_embedder_some_for_hybrid_with_key() {
        std::env::set_var("MCPGW_M2B_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"hybrid\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2B_KEY\"\n",
        )
        .unwrap();
        assert!(build_embedder(&cfg).unwrap().is_some());
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mcpgw build_embedder_some_for_hybrid_with_key`
Expected: FAIL —— 当前 `build_embedder` 对 `"hybrid"` 走 `_ => Ok(None)`，断言 `is_some()` 失败。

- [ ] **Step 3: 扩 match 臂 + 泛化消息**

在 `crates/mcpgw/src/main.rs` 的 `build_embedder` 中，把：

```rust
        "vector" => {
            let v = cfg
                .retrieval
                .vector
                .as_ref()
                .ok_or("strategy=\"vector\" requires [retrieval.vector]")?;
```

改为（匹配臂含 `hybrid`，消息含实际策略名）：

```rust
        "vector" | "hybrid" => {
            let v = cfg
                .retrieval
                .vector
                .as_ref()
                .ok_or_else(|| {
                    format!("strategy={:?} requires [retrieval.vector]", cfg.retrieval.strategy)
                })?;
```

（同臂内其余代码——读 key、建 `OpenAiEmbedder`、裹 `CachingEmbedder`——保持不变。）

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p mcpgw`
Expected: PASS（含新增 `build_embedder_some_for_hybrid_with_key` 与既有 `build_embedder_*`）。

- [ ] **Step 5: 提交**

```bash
git add crates/mcpgw/src/main.rs
git commit -m "feat(mcpgw): build_embedder builds embedder for hybrid too (M2-B T5)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 6: gateway hybrid 快照测试（镜像 vector 用例）

**Files:**
- Modify: `crates/gateway/src/lib.rs`（`tests` 模块新增一个测试）

- [ ] **Step 1: 新增测试**

在 `crates/gateway/src/lib.rs` 的 `tests` 模块，紧跟 `with_embedder_rebuild_builds_vector_snapshot_no_upstreams` 之后新增：

```rust
    #[tokio::test]
    async fn with_embedder_rebuild_builds_hybrid_snapshot_no_upstreams() {
        let state = super::GatewayState::with_embedder(
            "hybrid",
            std::sync::Arc::new(retrieval::MockEmbedder::new(16)),
        )
        .expect("hybrid state");
        // No upstreams -> empty catalog; rebuild must succeed (embed of [] is fine).
        state.rebuild_snapshot().await.expect("rebuild ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }
```

- [ ] **Step 2: 运行，确认通过**

Run: `cargo test -p gateway with_embedder_rebuild_builds_hybrid_snapshot_no_upstreams`
Expected: PASS（`build_strategy("hybrid", Some)` 经 Task 2 已可用；空目录 rebuild 成功）。

- [ ] **Step 3: 提交**

```bash
git add crates/gateway/src/lib.rs
git commit -m "test(gateway): hybrid snapshot rebuild with embedder (M2-B T6)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 7: 分层文档（L1–L4 + README + roadmap）

**Files:**
- Create: `docs/L4-api/retrieval-hybrid.md`
- Modify: `docs/L4-api/retrieval-lib.md`（订正 M2-A 漂移 + hybrid）、`docs/L4-api/config-lib.md`、`docs/L4-api/mcpgw-main.md`
- Modify: `docs/L3-details/retrieval.md`、`docs/L2-components/retrieval.md`、`docs/L1-overview.md`、`docs/README.md`
- Modify: `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`

- [ ] **Step 1: 新建 L4 `retrieval-hybrid.md`**

创建 `docs/L4-api/retrieval-hybrid.md`：

```markdown
# L4 — `crates/retrieval/src/hybrid.rs` API

源文件：`crates/retrieval/src/hybrid.rs`。

## `struct HybridStrategy`
\`\`\`rust
pub struct HybridStrategy { /* bm25, vector, doc_count（均私有） */ }

impl HybridStrategy {
    pub fn new(embedder: std::sync::Arc<dyn Embedder>) -> Self;
}
\`\`\`
RRF 混合检索器：内部组合一个 `Bm25Strategy` 与一个 `VectorStrategy`（后者持有 `embedder`、自带 BM25 降级）。
实现 `RetrievalStrategy`（async）。一般经 `build_strategy("hybrid", Some(embedder))` 构造。

- `index(&mut self, &Catalog)`：分别 `index` BM25 与向量分量，并记录 `doc_count`（= 目录工具数，用作全深度子检索的 `top_k`）。
- `search(&self, query, top_k)`：`doc_count == 0` → 空；否则取 `bm25.search(query, doc_count)` 与
  `vector.search(query, doc_count)` 两份**全深度**子排名，做 RRF 融合后取 `top_k`。

## 私有 `fn rrf_fuse` / 常量 `RRF_K`
- `const RRF_K: f32 = 60.0`（业界标准，**不暴露配置**）。
- `rrf_fuse(lists, top_k)`：对每份列表按名次 `rank`（从 1）累加 `1/(RRF_K + rank)`，按 `qualified_name`
  求和；按「融合分降序 + `qualified_name` 升序」排序后截到 `top_k`。`ScoredTool.score` 承载**融合分**
  （量级小，例如双表 rank1 ≈ `2/61`），**不可跨策略比较**。私有，不属公开 API。

## 行为要点
- BM25 子表只含命中词项的文档；向量子表含全部文档 → 仅语义相关者仍可经向量表进入融合（召回增益）。
- embedding 失败时向量分量返回内部 BM25 排名 → 两份子表≈同一 BM25 排名 → 融合后退化≈纯 BM25（自愈，无需额外标志）。

> 算法/数据流见 L3：[retrieval](../L3-details/retrieval.md)。
```

> 注：上面代码块用 `\`\`\`` 仅为在本 plan 内转义；写入文件时用正常三反引号 ```` ``` ````。

- [ ] **Step 2: 订正 L4 `retrieval-lib.md`（M2-A 漂移 + hybrid）**

把 `docs/L4-api/retrieval-lib.md` 中 `## trait RetrievalStrategy`、`## enum StrategyError`、`## fn build_strategy` 三节替换为：

```markdown
## `trait RetrievalStrategy`（`#[async_trait]`）
\`\`\`rust
#[async_trait]
pub trait RetrievalStrategy: Send + Sync {
    async fn index(&mut self, catalog: &catalog::Catalog);
    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}
\`\`\`
可插拔检索策略抽象（M2-A 起为 async）。`index` 从目录（重）建索引；`search` 返回最多 `top_k` 条、相关性降序。

## `enum StrategyError`
\`\`\`rust
#[derive(Debug, thiserror::Error)]
pub enum StrategyError {
    NotImplemented(String),   // 未知/未实现的策略名
    EmbedderRequired(String), // 策略需要 embedder 但未提供（"vector"/"hybrid" 且 embedder=None）
}
\`\`\`

## `fn build_strategy`
\`\`\`rust
pub fn build_strategy(
    name: &str,
    embedder: Option<&std::sync::Arc<dyn Embedder>>,
) -> Result<Box<dyn RetrievalStrategy>, StrategyError>
\`\`\`
按名构造策略：`"bm25"` → `Bm25Strategy`（无需 embedder）；`"vector"` → `VectorStrategy`（**要求** embedder，
否则 `EmbedderRequired`）；`"hybrid"` → `HybridStrategy`（**要求** embedder，否则 `EmbedderRequired`）；
其余 → `NotImplemented`。**接受 `&str`，使本 crate 不依赖 `config`**；embedder 以 `Option<&Arc<dyn Embedder>>`
注入，使本 crate 仍不引入 HTTP 依赖。
```

并把页脚的 `> 内部数据结构 IndexedDoc 为私有...` 一行后补：

```markdown
> 逐文件 hybrid API 见 [retrieval/hybrid.rs](./retrieval-hybrid.md)。
```

- [ ] **Step 3: 更新 L4 `config-lib.md` 与 `mcpgw-main.md`**

(a) `docs/L4-api/config-lib.md`：把 `validate` 描述里 `strategy="vector"` 时必须有 `[retrieval.vector]`
改为 `strategy ∈ {vector, hybrid}` 时必须有；同步改 `Invalid` 项列表中的同一处（两处都把
`strategy="vector"` 改为 `strategy ∈ {vector,hybrid}`）；`pub vector: Option<VectorConfig>` 的行内注释
`strategy="vector" 时必填` 改为 `strategy ∈ {vector,hybrid} 时必填`。

(b) `docs/L4-api/mcpgw-main.md`：把 `fn build_embedder` 行的 `"vector"` → 改述为 `"vector"/"hybrid"`：
`按 retrieval.strategy 建 embedder：vector/hybrid → 从 api_key_env 启动期 fail-fast 读 key ... 其它（bm25）→ None`。

- [ ] **Step 4: 更新 L3 `retrieval.md`（加 hybrid 小节）**

把标题 `# L3 — \`retrieval\` 细节（BM25 算法）` 改为 `# L3 — \`retrieval\` 细节（BM25 / 向量 / 混合）`，
并在文件末尾（向量策略小节之后）追加：

```markdown
## 混合策略 `HybridStrategy`（RRF）

`HybridStrategy`（`crates/retrieval/src/hybrid.rs`）用 **Reciprocal Rank Fusion** 融合 `Bm25Strategy`
（词法）与 `VectorStrategy`（语义）两份排名：

- **全深度子检索**：`search` 以 `doc_count`（索引时 = `catalog.iter().count()`）为 `top_k` 调用两个子策略，
  确保 RRF 看到每个文档在各列表中的**真实名次**；若先各自截到 `top_k` 再融合，会丢掉「一边名次低、另一边名次
  高」的单边命中。
- **RRF 公式**：`fused(doc) = Σ_L 1/(60 + rank_L(doc))`，`k=60` **固定**（不暴露配置）。按「融合分降序 +
  `qualified_name` 升序」排序后截 `top_k`。融合分量级很小，**不可跨策略比较**。
- **不对称性（有意）**：BM25 子表只含命中词项的文档（`score>0` 过滤）；向量子表含**全部**文档。故仅语义相关、
  无词法命中的工具仍能经向量表进入融合（hybrid 相对纯 BM25 的召回增益）；反之，对任意非空 query，含 embedding
  的 hybrid 总能返回最多 `top_k` 个语义最近结果（不同于纯 BM25「无命中即空」）。
- **降级自愈**：embedding 失败时（索引期或查询期），`VectorStrategy` 返回内部 BM25 排名 → 两份子表≈同一 BM25
  排名 → RRF 融合后名次单调一致 → hybrid 退化≈纯 BM25。**无需额外 degraded 标志**。
```

- [ ] **Step 5: 更新 L2 `retrieval.md`**

在 `docs/L2-components/retrieval.md` 做四处改动：
1. 「职责」段：在列出实现处补上 **`HybridStrategy`**（RRF 融合 BM25+向量；M2-B 新增）。
2. `### 错误 StrategyError` 的 `NotImplemented(String)` 行：去掉「`"hybrid"`（延后到 M2-B）」，改为
   `未知策略名`；`EmbedderRequired` 行补上 `/"hybrid"`：`（"vector"/"hybrid" 且 embedder 为 None）`。
3. `### 工厂 build_strategy` 的 `"hybrid"` 项：把 `→ NotImplemented（延后到 M2-B）` 改为
   `→ HybridStrategy，要求 embedder，否则 EmbedderRequired`；并新增一个 `### 类型 HybridStrategy` 小节：
   `RRF（k=60）融合内置 Bm25Strategy + VectorStrategy 的两份全深度排名；详见 L3 与 L4：retrieval-hybrid.md`。
4. 「向下导航」L4 列表追加 `· [retrieval/hybrid.rs](../L4-api/retrieval-hybrid.md)`。

- [ ] **Step 6: 更新 L1 `L1-overview.md`**

在 M2-A 相关描述处补一句：`M2-B 新增 hybrid（RRF 融合 BM25+向量），opt-in；**默认仍为 bm25**`。
（找到提及 M2-A / `strategy="vector"` 的段落，追加 hybrid 与「默认仍 bm25」的说明，勿改默认表述。）

- [ ] **Step 7: 更新 `docs/README.md` 索引**

1. L4 索引行末尾追加 `· [retrieval/hybrid.rs](./L4-api/retrieval-hybrid.md)`。
2. 底部里程碑覆盖说明：在 M2-A 之后补 **M2-B（hybrid RRF 融合）**。

- [ ] **Step 8: 更新 roadmap**

在 `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md` 的 M2 小节，对 **M2.T4 — HybridStrategy（RRF）**
标注「✅ 已完成（M2-B；默认仍 bm25，hybrid opt-in）」。

- [ ] **Step 9: 提交**

```bash
git add docs/
git commit -m "docs: L1-L4 + README + roadmap for M2-B hybrid retrieval (M2-B T7)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 8: 全量验证 + 合回 master

- [ ] **Step 1: 全量验证**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净；clippy 无告警；所有测试 PASS（含 retrieval 的 `hybrid` testkit 目标；`#[ignore]` 的真实冒烟仍被跳过）。

> 若 `cargo fmt --all --check` 报告与本次改动无关的既有文件（见用户记忆中 rustfmt 1.9.0 漂移），**只**核对本次改动文件本身 rustfmt 干净：`cargo fmt -- --check crates/retrieval/src/hybrid.rs`。

- [ ] **Step 2: 收尾（全部任务完成后）**

1. 派发最终整体 code review（spec 覆盖、RRF 正确性、降级、全深度融合、文档同步、无回归）。
2. 处理 blocking 项（如有）。
3. 用 superpowers:finishing-a-development-branch 把 `feat/m2b-hybrid-retrieval` 合回 master
   （`--no-ff`，本地），删分支。

## 实现期需现场确认/可能回退的点（spec §9）
- BM25 `score>0` 过滤使 BM25 子表不对称（仅命中词项）——RRF 下为预期，已由 `hybrid_surfaces_vector_candidates_when_bm25_empty` 固化。
- 含 embedding 时向量分量对**全部**文档排名，故 hybrid 对任意非空 query 总返回最多 `top_k`——属语义召回预期，文档已注明。
- `doc_count` 取 `catalog.iter().count()`，与 BM25/向量分量实际索引文档数一致（二者均逐 `catalog.iter()` 无过滤建索引）。
- `retrieval-lib.md` 的 M2-A 漂移（同步 trait/旧签名）在 T7 一并订正。
