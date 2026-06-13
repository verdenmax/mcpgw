# M2-A: 向量检索基础（异步策略 + Embedder + 缓存 + VectorStrategy）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把检索升级为「可插拔、异步、失败可降级」，并落地真正可用的云向量检索：`RetrievalStrategy` 改 async，新增 `Embedder`/`CachingEmbedder`/`VectorStrategy`（暴力余弦 + 内置 BM25 降级）与真实 OpenAI-兼容 embedder，`strategy = "vector"` 即可用。

**Architecture:** `retrieval` crate 暴露 async `RetrievalStrategy`（经 `async-trait` 对象安全）、`Embedder` 抽象、`VectorStrategy`、`CachingEmbedder`、`MockEmbedder`；HTTP-backed `OpenAiEmbedder` 独立在新 `embedder` crate（retrieval 保持无 HTTP 依赖）。`GatewayState` 注入 `Option<Arc<dyn Embedder>>`，每次 `rebuild_snapshot` 异步构建策略；缓存随 state 跨 rebuild 存活，只嵌新增工具。

**Tech Stack:** Rust 2021 / `async-trait` 0.1 / reqwest 0.13（仅 `embedder` crate）/ tokio / serde。

**Spec:** `docs/superpowers/specs/2026-06-13-mcpgw-m2a-vector-retrieval-design.md`

---

## 已确认的关键事实（实现时照用）

- `RetrievalStrategy` 当前是 **同步** trait（`fn index(&mut self, &Catalog)` / `fn search(&self, &str, usize) -> Vec<ScoredTool>`），存于 `metatools::GatewaySnapshot.strategy: Box<dyn RetrievalStrategy>`。
- `build_strategy(strategy: &str) -> Result<Box<dyn RetrievalStrategy>, StrategyError>`（"bm25" ok；其它 `NotImplemented`）。
- `GatewayState::new(strategy_name)` 当前在构造时调 `strat.index(&empty)`（同步）；`rebuild_snapshot`（async）调 `strat.index(&catalog)`（同步）。
- `metatools::search_tools(snap, query, top_k)` 同步；唯一调用处 `downstream::GatewayServer::call_tool` 的 `"search_tools"` 臂**已是 async**（直接 `await` 即可，无外溢）。
- CLI `mcpgw search`（`crates/mcpgw/src/main.rs` 的 `Command::Search` 臂，在同步 `run()` 内）也调 `strat.search(...)`；改用 `tokio::runtime` 包一层（`Serve` 臂已有先例）。
- `async-trait` 默认把方法 future 装箱为 `Send`；`Box<dyn RetrievalStrategy>` 因此保持对象安全。search future 跨 await 持有 `Arc<GatewaySnapshot>`，需 `Send`（满足）。
- `metatools`、`gateway` 已有 `tokio = { features=["full"] }` dev-dep；`retrieval` 需新增 tokio dev-dep 以跑 async 测试。

## File Structure

| 文件 | 职责 | 任务 |
|------|------|------|
| `Cargo.toml`(workspace) | 加 `async-trait = "0.1"`（+ 后续 reqwest 已在用） | T1/T7 |
| `crates/retrieval/Cargo.toml` | 加 `async-trait`；dev 加 `tokio`（rt+macros）；后续 `testkit` feature | T1/T2 |
| `crates/retrieval/src/lib.rs` | 拆分前的总入口：trait 改 async、`build_strategy` 改签名、re-export | T1/T5 |
| `crates/retrieval/src/embedder.rs`(新) | `Embedder` trait + `EmbedError` + `MockEmbedder`(testkit) | T2 |
| `crates/retrieval/src/caching.rs`(新) | `CachingEmbedder` 装饰器 + 内容哈希缓存 | T3 |
| `crates/retrieval/src/vector.rs`(新) | `VectorStrategy`（归一化余弦 + 内置 BM25 + 双降级） | T4 |
| `crates/retrieval/tests/golden.rs` | 改 async（`#[tokio::test]`） | T1 |
| `crates/metatools/src/tools.rs` | `search_tools` 改 async | T1 |
| `crates/gateway/src/lib.rs` | index/search 异步化；`with_embedder` 注入；rebuild 用 vector | T1/T5 |
| `crates/downstream/src/lib.rs` | search 臂 `await` | T1 |
| `crates/mcpgw/src/main.rs` | CLI search 用 runtime；启动期建 embedder + 注入 | T1/T5/T7/T8 |
| `crates/config/src/lib.rs` | `[retrieval.vector]` 结构 + 校验 | T7 |
| `crates/embedder/`(新 crate) | `OpenAiEmbedder`(reqwest) + mock-HTTP 单测 | T6 |
| `scripts/`, `crates/mcpgw/tests/` | 验证脚本 + 门控真实冒烟 | T8 |
| `docs/L1-L4` | 分层文档随各任务同提交，T9 收口 | T1–T9 |

> 任务顺序：T1(async 重构) → T2(Embedder) → T3(Caching) → T4(VectorStrategy) → T5(gateway 注入) → T6(真 embedder crate) → T7(config + mcpgw 装配) → T8(脚本+冒烟) → T9(文档收口)。

---

## Task 1: 把 `RetrievalStrategy` 改为 async（全链路 await，旧测试转绿）

原子重构：trait + `Bm25Strategy` + 所有调用点（metatools/gateway/downstream/mcpgw）+ 所有受影响测试一起改，使整个工作区编译通过、测试全绿。**不新增任何检索功能**（Embedder/Vector 在后续任务）。

**Files:**
- Modify: `Cargo.toml`(workspace), `crates/retrieval/Cargo.toml`
- Modify: `crates/retrieval/src/lib.rs`, `crates/retrieval/tests/golden.rs`
- Modify: `crates/metatools/src/tools.rs`, `crates/gateway/src/lib.rs`, `crates/downstream/src/lib.rs`, `crates/mcpgw/src/main.rs`

- [ ] **Step 1: 加依赖**

`Cargo.toml`（workspace `[workspace.dependencies]`）追加：
```toml
async-trait = "0.1"
```
`crates/retrieval/Cargo.toml`：`[dependencies]` 加 `async-trait`，`[dev-dependencies]` 加 tokio：
```toml
[dependencies]
catalog = { path = "../catalog" }
thiserror = { workspace = true }
async-trait = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros"] }
```

- [ ] **Step 2: 把 trait 与 `Bm25Strategy` 改 async**

`crates/retrieval/src/lib.rs`：顶部 `use catalog::Catalog;` 下加：
```rust
use async_trait::async_trait;
```
trait 定义改为：
```rust
/// A pluggable tool-retrieval strategy (BM25, vector, hybrid, ...).
#[async_trait]
pub trait RetrievalStrategy: Send + Sync {
    /// (Re)build internal indices from the current catalog.
    async fn index(&mut self, catalog: &Catalog);
    /// Return up to `top_k` tools relevant to `query`, best first.
    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}
```
`impl RetrievalStrategy for Bm25Strategy` 改为（函数体逐字不变，仅加 `#[async_trait]` 与 `async`）：
```rust
#[async_trait]
impl RetrievalStrategy for Bm25Strategy {
    async fn index(&mut self, catalog: &Catalog) {
        // ...（原同步实现体，逐字保留）...
    }
    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        // ...（原同步实现体，逐字保留）...
    }
}
```
> `build_strategy(strategy: &str)` 签名本任务**不变**（embedder 参数在 T5 加）；返回的 `Box<dyn RetrievalStrategy>` 现在方法是 async。

- [ ] **Step 3: 转 retrieval 自身测试为 async**

`crates/retrieval/src/lib.rs` 的 `mod tests`：把调用 `index`/`search` 的 3 个测试由 `#[test]` 改为 `#[tokio::test] async`，并对 `index(...)`/`search(...)` 加 `.await`：
- `bm25_ranks_relevant_tool_first`：`s.index(&sample_catalog()).await;` 和 `s.search("create github issue", 3).await`。
- `bm25_respects_top_k_and_filters_zero_score`：`s.index(...).await;`，每个 `s.search(...).await`。
- `build_strategy_returns_bm25_and_indexes`：`strat.index(&sample_catalog()).await;`、`strat.search("forecast", 8).await`。
`tokenize_splits_on_non_alphanumeric_and_lowercases` 与 `build_strategy_errors_on_unimplemented_strategies` 不调 index/search，**保持 `#[test]` 不变**。

`crates/retrieval/tests/golden.rs`：`golden_top_one_matches_expected` 改 `#[tokio::test] async`，`s.index(&load_catalog()).await;` 与 `s.search(query, 5).await`。

- [ ] **Step 4: 转调用点 —— metatools**

`crates/metatools/src/tools.rs`：`search_tools` 改 async：
```rust
/// Search the snapshot's tools for `query`, returning up to `top_k` summaries (best first).
pub async fn search_tools(snap: &GatewaySnapshot, query: &str, top_k: usize) -> Vec<ToolSummary> {
    snap.strategy
        .search(query, top_k)
        .await
        .into_iter()
        .map(|hit| ToolSummary {
            name: hit.qualified_name,
            description: hit.description,
        })
        .collect()
}
```
其 `mod tests` 里 `snapshot()` 辅助中 `strat.index(&catalog)` 需 await，但辅助是同步 `fn`：把 `snapshot()` 改 `async fn snapshot()`，内部 `strat.index(&catalog).await;`；`search_tools_returns_namespaced_summaries` 改 `#[tokio::test] async`，`let snap = snapshot().await;` 且 `search_tools(&snap, "weather forecast", 5).await`。`get_tool_details_returns_full_def_or_none` 与 `..._double_underscore` 也用了 `strat.index` —— 同样改 `#[tokio::test] async` 并对 `index` 加 `.await`（这两个不调 search_tools，但仍需 await index）。

- [ ] **Step 5: 转调用点 —— gateway**

`crates/gateway/src/lib.rs`：
- `new`：**移除** `strat.index(&empty);` 这一行（构造时不再 index；空目录搜索本就返回空，首个真实快照由异步 `rebuild_snapshot` 建）。同时把 `let mut strat = build_strategy(...)` 改为 `let strat = build_strategy(...)`（不再 index，避免 `clippy::unused_mut`）。`let empty = Catalog::new();` 与 `GatewaySnapshot::new(empty, strat)` 保留。
- `rebuild_snapshot`：`strat.index(&catalog);` 改为 `strat.index(&catalog).await;`。
- 若该文件 `mod tests` 中有同步调用 `strat.index`/`search` 或对 `search_tools` 的调用，逐一加 `.await` 并把相应测试改 `#[tokio::test] async`（gateway 已有 tokio full dev-dep）。

- [ ] **Step 6: 转调用点 —— downstream**

`crates/downstream/src/lib.rs` 的 `call_tool` `"search_tools"` 臂：
```rust
let hits = metatools::search_tools(&snap, query, top_k).await;
```
（该臂已是 `async fn call_tool`，仅加 `.await`。）

- [ ] **Step 7: 转调用点 —— mcpgw CLI search**

`crates/mcpgw/src/main.rs` 的 `Command::Search` 臂当前在同步 `run()` 内同步调用 `strat.index`/`strat.search`。改为在该臂内建一个临时 runtime 并 `block_on`：
```rust
Command::Search { query, top_k } => {
    let catalog = load_catalog(&cli.catalog)?;
    let mut strat = build_strategy(&cfg.retrieval.strategy).map_err(|e| e.to_string())?;
    let k = top_k.unwrap_or(cfg.retrieval.top_k);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let hits = rt.block_on(async {
        strat.index(&catalog).await;
        strat.search(&query, k).await
    });
    let out: Vec<_> = hits
        .iter()
        .map(|h| {
            serde_json::json!({
                "name": h.qualified_name,
                "description": h.description,
                "score": h.score,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
```
`crates/mcpgw/src/main.rs` 的 `mod tests` 中 `run_serve_builds_initial_snapshot_with_no_upstreams` 调 `metatools::search_tools(&state.snapshot(), "anything", 5)` —— 改为 `metatools::search_tools(&state.snapshot(), "anything", 5).await`（该测试已是 `#[tokio::test]`）。

- [ ] **Step 8: 全量构建 + 测试，确认通过**

Run:
```bash
cargo build --workspace
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
```
Expected: 全工作区编译通过；所有测试 PASS（计数与重构前一致，golden/bm25 行为不变）；clippy/fmt 干净。

- [ ] **Step 9: 文档（L3 retrieval）**

`docs/L3-details/retrieval.md`：补一段「检索 trait 已异步化（`async-trait`，保持 `Box<dyn>` 对象安全）；`GatewayState::new` 不再在构造期 index、首个真实快照由异步 rebuild 建；`search_tools`/`strategy.search` 全链路 await」。

- [ ] **Step 10: 提交**

```bash
git add Cargo.toml crates/retrieval crates/metatools/src/tools.rs crates/gateway/src/lib.rs crates/downstream/src/lib.rs crates/mcpgw/src/main.rs docs/L3-details/retrieval.md
git commit -m "refactor(retrieval): make RetrievalStrategy async (async-trait) end-to-end (M2-A T1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: `Embedder` trait + `EmbedError` + `MockEmbedder`（testkit）

新增嵌入抽象与确定性 mock。mock 用「token 哈希分桶」生成稳定伪向量，使共享 token 的文本余弦更高，便于后续断言排序；并带调用计数与已嵌文本记录，供缓存测试用。

**Files:**
- Create: `crates/retrieval/src/embedder.rs`
- Modify: `crates/retrieval/src/lib.rs`（`mod embedder` + re-export）, `crates/retrieval/Cargo.toml`（`testkit` feature）
- Create: `crates/retrieval/tests/embedder.rs`（`required-features = ["testkit"]`）

- [ ] **Step 1: 加 `testkit` feature + test target**

`crates/retrieval/Cargo.toml`：在 `[package]` 后、`[dependencies]` 前加：
```toml
[features]
testkit = []
```
文件末尾加：
```toml
[[test]]
name = "embedder"
required-features = ["testkit"]
```

- [ ] **Step 2: 写失败测试**

`crates/retrieval/tests/embedder.rs`（新建）：
```rust
use retrieval::{Embedder, MockEmbedder};

#[tokio::test]
async fn mock_embedder_is_deterministic_and_right_dim() {
    let e = MockEmbedder::new(64);
    assert_eq!(e.dim(), 64);
    let a = e.embed(&["create github issue".to_string()]).await.unwrap();
    let b = e.embed(&["create github issue".to_string()]).await.unwrap();
    assert_eq!(a, b, "same text -> same vector");
    assert_eq!(a[0].len(), 64);
}

#[tokio::test]
async fn mock_embedder_shared_tokens_score_higher_cosine() {
    let e = MockEmbedder::new(64);
    let v = e
        .embed(&[
            "send a slack message".to_string(),   // query
            "post a message to a slack channel".to_string(), // related
            "get the weather forecast".to_string(),          // unrelated
        ])
        .await
        .unwrap();
    let cos = |x: &[f32], y: &[f32]| -> f32 {
        let dot: f32 = x.iter().zip(y).map(|(a, b)| a * b).sum();
        let nx: f32 = x.iter().map(|a| a * a).sum::<f32>().sqrt();
        let ny: f32 = y.iter().map(|a| a * a).sum::<f32>().sqrt();
        dot / (nx * ny)
    };
    assert!(
        cos(&v[0], &v[1]) > cos(&v[0], &v[2]),
        "related text must be closer than unrelated"
    );
}

#[tokio::test]
async fn mock_embedder_failing_returns_err() {
    let e = MockEmbedder::failing(64);
    assert!(e.embed(&["x".to_string()]).await.is_err());
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p retrieval --features testkit --test embedder`
Expected: 编译失败（`Embedder`/`MockEmbedder` 未定义）。

- [ ] **Step 4: 实现 `embedder.rs`**

`crates/retrieval/src/embedder.rs`（新建）：
```rust
//! The `Embedder` abstraction: turn texts into vectors. The HTTP-backed provider lives in
//! the separate `embedder` crate; this module only defines the trait, errors, and a
//! deterministic `MockEmbedder` (behind the `testkit` feature) for tests.

use async_trait::async_trait;

/// Errors from embedding. Kept provider-agnostic so `retrieval` needs no HTTP dependency.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding provider error: {0}")]
    Provider(String),
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    Dimension { expected: usize, got: usize },
}

/// Turns a batch of texts into one vector each (same order). All-or-nothing per call.
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// Expected embedding dimension (used for sanity checks).
    fn dim(&self) -> usize;
}

#[cfg(feature = "testkit")]
mod mock {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Deterministic test embedder. Each token (split on non-alphanumeric, lowercased) is
    /// hashed into one of `dim` buckets and adds 1.0 there — so texts sharing tokens have
    /// higher cosine similarity. Records call count + texts seen for cache assertions.
    pub struct MockEmbedder {
        dim: usize,
        fail: bool,
        pub calls: Arc<AtomicUsize>,
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    impl MockEmbedder {
        pub fn new(dim: usize) -> Self {
            Self {
                dim,
                fail: false,
                calls: Arc::new(AtomicUsize::new(0)),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
        /// An embedder whose `embed` always errors (drives degradation tests).
        pub fn failing(dim: usize) -> Self {
            Self {
                fail: true,
                ..Self::new(dim)
            }
        }
        fn vec_for(&self, text: &str) -> Vec<f32> {
            let mut v = vec![0.0f32; self.dim];
            for tok in crate::tokenize(text) {
                // FNV-1a over the token bytes -> bucket.
                let mut h: u64 = 0xcbf29ce484222325;
                for b in tok.as_bytes() {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                v[(h as usize) % self.dim] += 1.0;
            }
            v
        }
    }

    #[async_trait]
    impl Embedder for MockEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(EmbedError::Provider("mock failure".into()));
            }
            self.seen.lock().unwrap().extend(texts.iter().cloned());
            Ok(texts.iter().map(|t| self.vec_for(t)).collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }
}

#[cfg(feature = "testkit")]
pub use mock::MockEmbedder;
```

`crates/retrieval/src/lib.rs`：在文件靠前的模块声明处加（与现有 `use` 同级）：
```rust
mod embedder;
pub use embedder::{EmbedError, Embedder};
#[cfg(feature = "testkit")]
pub use embedder::MockEmbedder;
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p retrieval --features testkit --test embedder`
Expected: 3 个测试 PASS。也跑 `cargo test -p retrieval`（无 feature）确认默认构建仍编译、原测试绿。

- [ ] **Step 6: 文档（L4 + L2 retrieval）**

- 新建 `docs/L4-api/retrieval-embedder.md`：`Embedder` trait、`EmbedError`、`MockEmbedder`(testkit) 的语义。
- `docs/L2-components/retrieval.md`：新增「Embedder 抽象（HTTP 实现在独立 embedder crate）」一节。

- [ ] **Step 7: 提交**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/src/embedder.rs crates/retrieval/src/lib.rs crates/retrieval/tests/embedder.rs docs/L4-api/retrieval-embedder.md docs/L2-components/retrieval.md
git commit -m "feat(retrieval): Embedder trait + EmbedError + MockEmbedder (M2-A T2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: `CachingEmbedder`（内容哈希缓存装饰器）

包装任意 `Arc<dyn Embedder>`，按文本内容哈希缓存向量；仅对未命中文本调用内层，按原顺序还原。跨 rebuild 复用由 `GatewayState` 持有 `Arc<CachingEmbedder>` 实现（T5）。

**Files:**
- Create: `crates/retrieval/src/caching.rs`
- Modify: `crates/retrieval/src/lib.rs`（`mod caching` + re-export）, `crates/retrieval/Cargo.toml`（test target）
- Create: `crates/retrieval/tests/caching.rs`（`required-features = ["testkit"]`）

- [ ] **Step 1: 加 test target**

`crates/retrieval/Cargo.toml` 末尾加：
```toml
[[test]]
name = "caching"
required-features = ["testkit"]
```

- [ ] **Step 2: 写失败测试**

`crates/retrieval/tests/caching.rs`（新建）：
```rust
use retrieval::{CachingEmbedder, Embedder, MockEmbedder};
use std::sync::Arc;

#[tokio::test]
async fn caches_and_only_embeds_new_texts() {
    let mock = MockEmbedder::new(32);
    let seen = mock.seen.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));

    let v1 = caching.embed(&["a".into(), "b".into()]).await.unwrap();
    let v2 = caching.embed(&["a".into(), "c".into()]).await.unwrap(); // "a" cached, "c" new

    // Inner saw each unique text exactly once, in first-seen order.
    assert_eq!(*seen.lock().unwrap(), vec!["a", "b", "c"]);
    // Cached vector for "a" is identical across calls.
    assert_eq!(v1[0], v2[0]);
}

#[tokio::test]
async fn preserves_input_order_and_dedups_within_a_call() {
    let mock = MockEmbedder::new(16);
    let seen = mock.seen.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));
    assert_eq!(caching.dim(), 16);

    let v = caching
        .embed(&["x".into(), "y".into(), "x".into()])
        .await
        .unwrap();
    assert_eq!(v.len(), 3);
    assert_eq!(v[0], v[2]); // same text -> same vector, original order preserved
    assert_eq!(*seen.lock().unwrap(), vec!["x", "y"]); // "x" embedded once
}

#[tokio::test]
async fn all_cached_second_call_skips_inner() {
    let mock = MockEmbedder::new(8);
    let calls = mock.calls.clone();
    let caching = CachingEmbedder::new(Arc::new(mock));
    caching.embed(&["a".into()]).await.unwrap();
    caching.embed(&["a".into()]).await.unwrap(); // fully cached
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn propagates_inner_error() {
    let caching = CachingEmbedder::new(Arc::new(MockEmbedder::failing(8)));
    assert!(caching.embed(&["x".into()]).await.is_err());
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p retrieval --features testkit --test caching`
Expected: 编译失败（`CachingEmbedder` 未定义）。

- [ ] **Step 4: 实现 `caching.rs`**

`crates/retrieval/src/caching.rs`（新建）：
```rust
//! `CachingEmbedder`: an `Embedder` decorator that memoizes vectors by text content hash,
//! so repeated/unchanged tool texts are embedded only once across snapshot rebuilds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::embedder::{EmbedError, Embedder};

fn hash_text(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Memoizes embeddings by content hash. Only cache-miss texts are forwarded to `inner`.
pub struct CachingEmbedder {
    inner: Arc<dyn Embedder>,
    cache: Mutex<HashMap<u64, Arc<[f32]>>>,
}

impl CachingEmbedder {
    pub fn new(inner: Arc<dyn Embedder>) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Embedder for CachingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let hashes: Vec<u64> = texts.iter().map(|t| hash_text(t)).collect();

        // Collect unique cache-miss texts, preserving first-seen order.
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        {
            let cache = self.cache.lock().unwrap();
            for (h, t) in hashes.iter().zip(texts) {
                if !cache.contains_key(h) && miss_seen.insert(*h) {
                    miss_texts.push(t.clone());
                }
            }
        }

        // Embed only the misses (skip the call entirely if everything is cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            let mut cache = self.cache.lock().unwrap();
            for (t, v) in miss_texts.iter().zip(embedded) {
                cache.insert(hash_text(t), Arc::from(v.into_boxed_slice()));
            }
        }

        // Reassemble in original input order.
        let cache = self.cache.lock().unwrap();
        Ok(hashes
            .iter()
            .map(|h| cache.get(h).expect("just inserted/hit").to_vec())
            .collect())
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }
}
```

`crates/retrieval/src/lib.rs`：模块声明处加：
```rust
mod caching;
pub use caching::CachingEmbedder;
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p retrieval --features testkit`
Expected: caching(4) + embedder(3) + lib/golden 原测试全 PASS。

- [ ] **Step 6: 文档（L4 + L3 retrieval）**

- `docs/L4-api/retrieval-embedder.md`：补 `CachingEmbedder::new` 与缓存语义（内容哈希、仅嵌未命中、保序、全命中跳过内层、错误不缓存）。
- `docs/L3-details/retrieval.md`：补「缓存键=文本内容哈希，跨 rebuild 复用，list_changed 只嵌新增」。

- [ ] **Step 7: 提交**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/src/caching.rs crates/retrieval/src/lib.rs crates/retrieval/tests/caching.rs docs/L4-api/retrieval-embedder.md docs/L3-details/retrieval.md
git commit -m "feat(retrieval): CachingEmbedder content-hash memoization (M2-A T3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: `VectorStrategy`（归一化余弦 + 内置 BM25 + 双降级）

实现向量策略：索引期同时建 BM25 与（归一化）向量；查询期余弦排序；索引/查询任一嵌入失败透明降级 BM25。

**Files:**
- Create: `crates/retrieval/src/vector.rs`
- Modify: `crates/retrieval/src/lib.rs`（`mod vector` + re-export）, `crates/retrieval/Cargo.toml`（`tracing` dep + test target）
- Create: `crates/retrieval/tests/vector.rs`（`required-features = ["testkit"]`）

- [ ] **Step 1: 加依赖 + test target**

`crates/retrieval/Cargo.toml`：`[dependencies]` 加 `tracing = { workspace = true }`；末尾加：
```toml
[[test]]
name = "vector"
required-features = ["testkit"]
```

- [ ] **Step 2: 写失败测试**

`crates/retrieval/tests/vector.rs`（新建）：
```rust
use catalog::{Catalog, ToolDef};
use retrieval::{MockEmbedder, RetrievalStrategy, VectorStrategy};
use serde_json::Value;
use std::sync::Arc;

fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
    ToolDef { server: server.into(), name: name.into(), description: desc.into(), input_schema: Value::Null }
}

fn catalog() -> Catalog {
    Catalog::from_tooldefs(vec![
        tool("slack", "post_message", "Send a chat message to a Slack channel"),
        tool("weather", "get_forecast", "Get the weather forecast for a location"),
        tool("github", "create_issue", "Create a new issue in a GitHub repository"),
    ])
}

#[tokio::test]
async fn ranks_by_cosine_similarity() {
    let mut s = VectorStrategy::new(Arc::new(MockEmbedder::new(128)));
    s.index(&catalog()).await;
    let hits = s.search("send chat message slack channel", 3).await;
    assert_eq!(hits[0].qualified_name, "slack__post_message");
    // sorted descending
    for w in hits.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
}

#[tokio::test]
async fn truncates_to_top_k() {
    let mut s = VectorStrategy::new(Arc::new(MockEmbedder::new(64)));
    s.index(&catalog()).await;
    assert_eq!(s.search("message", 1).await.len(), 1);
}

#[tokio::test]
async fn degrades_to_bm25_when_index_embedding_fails() {
    let mut s = VectorStrategy::new(Arc::new(MockEmbedder::failing(64)));
    s.index(&catalog()).await; // embed fails at index -> degraded, BM25 still built
    let hits = s.search("forecast", 5).await; // served by the built-in BM25
    assert_eq!(hits[0].qualified_name, "weather__get_forecast");
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p retrieval --features testkit --test vector`
Expected: 编译失败（`VectorStrategy` 未定义）。

- [ ] **Step 4: 实现 `vector.rs`**

`crates/retrieval/src/vector.rs`（新建）：
```rust
//! `VectorStrategy`: brute-force cosine retrieval over cloud embeddings, with a built-in
//! `Bm25Strategy` it transparently falls back to when embeddings are unavailable (either the
//! index-time batch embed failed, or a per-query embed fails). The tool catalog is small, so
//! a linear scan over normalized vectors (cosine == dot product) is plenty.

use std::sync::Arc;

use async_trait::async_trait;
use catalog::Catalog;

use crate::embedder::Embedder;
use crate::{Bm25Strategy, RetrievalStrategy, ScoredTool};

/// L2-normalize in place; a zero vector is left as-is (its cosine with anything is 0).
fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The text embedded per tool: qualified name + description.
fn tool_text(t: &catalog::ToolDef) -> String {
    format!("{}\n{}", t.qualified_name(), t.description)
}

pub struct VectorStrategy {
    embedder: Arc<dyn Embedder>,
    bm25: Bm25Strategy,
    /// (qualified_name, description, normalized embedding) — empty when degraded.
    vectors: Vec<(String, String, Vec<f32>)>,
    degraded: bool,
}

impl VectorStrategy {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            embedder,
            bm25: Bm25Strategy::new(),
            vectors: Vec::new(),
            degraded: false,
        }
    }
}

#[async_trait]
impl RetrievalStrategy for VectorStrategy {
    async fn index(&mut self, catalog: &Catalog) {
        // Always (re)build the BM25 fallback first.
        self.bm25 = Bm25Strategy::new();
        self.bm25.index(catalog).await;

        let tools: Vec<&catalog::ToolDef> = catalog.iter().collect();
        let texts: Vec<String> = tools.iter().map(|t| tool_text(t)).collect();
        match self.embedder.embed(&texts).await {
            Ok(vecs) => {
                self.vectors = tools
                    .iter()
                    .zip(vecs)
                    .map(|(t, v)| (t.qualified_name(), t.description.clone(), normalize(v)))
                    .collect();
                self.degraded = false;
            }
            Err(e) => {
                tracing::warn!(error = %e, "vector index embedding failed; degrading to BM25");
                self.vectors.clear();
                self.degraded = true;
            }
        }
    }

    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool> {
        if self.degraded || self.vectors.is_empty() {
            return self.bm25.search(query, top_k).await;
        }
        let qv = match self.embedder.embed(&[query.to_string()]).await {
            Ok(mut v) => normalize(v.remove(0)),
            Err(e) => {
                tracing::warn!(error = %e, "vector query embedding failed; falling back to BM25");
                return self.bm25.search(query, top_k).await;
            }
        };

        let mut scored: Vec<ScoredTool> = self
            .vectors
            .iter()
            .map(|(qname, desc, v)| ScoredTool {
                qualified_name: qname.clone(),
                description: desc.clone(),
                score: dot(&qv, v),
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
}
```

`crates/retrieval/src/lib.rs`：模块声明处加：
```rust
mod vector;
pub use vector::VectorStrategy;
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p retrieval --features testkit`
Expected: vector(3) + caching(4) + embedder(3) + lib/golden 全 PASS。

- [ ] **Step 6: 文档（L4 + L3 retrieval）**

- 新建 `docs/L4-api/retrieval-vector.md`：`VectorStrategy::new`、index/search 语义、归一化余弦、内置 BM25 与双降级路径。
- `docs/L3-details/retrieval.md`：补「VectorStrategy：归一化后余弦=点积、暴力线性扫描（目录小）、degraded 与 per-query 两条降级」。

- [ ] **Step 7: 提交**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/src/vector.rs crates/retrieval/src/lib.rs crates/retrieval/tests/vector.rs docs/L4-api/retrieval-vector.md docs/L3-details/retrieval.md
git commit -m "feat(retrieval): VectorStrategy cosine + built-in BM25 degradation (M2-A T4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: `build_strategy(name, embedder)` + `GatewayState` 注入 embedder

让工厂可构建 vector，`GatewayState` 持有可选 `Arc<dyn Embedder>`，rebuild 用它建策略。

**Files:**
- Modify: `crates/retrieval/src/lib.rs`（`build_strategy` 签名 + `StrategyError`）
- Modify: `crates/gateway/src/lib.rs`（embedder 字段 + `with_embedder` + rebuild）, `crates/gateway/Cargo.toml`（dev `retrieval/testkit`）
- Modify: `crates/mcpgw/src/main.rs`（search 臂 `build_strategy` 传 `None`）

- [ ] **Step 1: 改 `build_strategy` 签名 + 新错误（先改实现，再修调用点；本步先让 retrieval 编译）**

`crates/retrieval/src/lib.rs`：
```rust
#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("retrieval strategy {0:?} is not implemented in this version")]
    NotImplemented(String),
    #[error("retrieval strategy {0:?} requires an embedder but none was configured")]
    EmbedderRequired(String),
}

/// Construct a retrieval strategy by name. "vector" requires `embedder`; "hybrid" is M2-B.
pub fn build_strategy(
    name: &str,
    embedder: Option<&std::sync::Arc<dyn Embedder>>,
) -> Result<Box<dyn RetrievalStrategy>, StrategyError> {
    match name {
        "bm25" => Ok(Box::new(Bm25Strategy::new())),
        "vector" => match embedder {
            Some(e) => Ok(Box::new(VectorStrategy::new(e.clone()))),
            None => Err(StrategyError::EmbedderRequired(name.to_string())),
        },
        other => Err(StrategyError::NotImplemented(other.to_string())),
    }
}
```

- [ ] **Step 2: 修 retrieval 自身的 `build_strategy` 测试**

`crates/retrieval/src/lib.rs` 的 `mod tests`：
- `build_strategy_returns_bm25_and_indexes`：`build_strategy("bm25", None)`。
- `build_strategy_errors_on_unimplemented_strategies` 改为：
```rust
    #[test]
    fn build_strategy_errors_appropriately() {
        assert!(matches!(
            build_strategy("hybrid", None),
            Err(StrategyError::NotImplemented(_))
        ));
        assert!(matches!(
            build_strategy("vector", None),
            Err(StrategyError::EmbedderRequired(_))
        ));
    }
```
新增（testkit 下，放 `tests/vector.rs` 里更合适——在该文件加）：
```rust
#[tokio::test]
async fn build_strategy_vector_with_embedder_works() {
    use retrieval::{build_strategy, Embedder};
    let e: std::sync::Arc<dyn Embedder> = std::sync::Arc::new(MockEmbedder::new(32));
    let mut strat = build_strategy("vector", Some(&e)).expect("vector with embedder");
    strat.index(&catalog()).await;
    assert!(!strat.search("forecast", 5).await.is_empty());
}
```

- [ ] **Step 3: 改 gateway —— embedder 字段 + 构造 + rebuild**

`crates/gateway/src/lib.rs`：
- 顶部加 `use std::sync::Arc;`（若已存在则跳过）与 `use retrieval::Embedder;`。
- `GatewayState` 结构加字段：
```rust
    embedder: Option<Arc<dyn Embedder>>,
```
- `new` 改为传 `None` 并设 `embedder: None`：
```rust
    pub fn new(strategy_name: &str) -> Result<Self, GatewayError> {
        let strat = build_strategy(strategy_name, None)
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        let empty = Catalog::new();
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
            embedder: None,
            rebuild_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Create state whose retrieval strategy is backed by `embedder` (for "vector"/"hybrid").
    pub fn with_embedder(
        strategy_name: &str,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self, GatewayError> {
        let strat = build_strategy(strategy_name, Some(&embedder))
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        let empty = Catalog::new();
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
            embedder: Some(embedder),
            rebuild_lock: Arc::new(Mutex::new(())),
        })
    }
```
- `rebuild_snapshot` 里：
```rust
        let mut strat = build_strategy(&self.strategy_name, self.embedder.as_ref())
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        strat.index(&catalog).await;
```

- [ ] **Step 4: 改 mcpgw CLI search —— 传 None**

`crates/mcpgw/src/main.rs` 的 `Command::Search` 臂中 `build_strategy(&cfg.retrieval.strategy)` 改为 `build_strategy(&cfg.retrieval.strategy, None)`（CLI 离线 search 仅支持 bm25；vector 经 `serve`，T6 装配 embedder）。

- [ ] **Step 5: 加 gateway 测试**

`crates/gateway/Cargo.toml` `[dev-dependencies]` 加 `retrieval = { path = "../retrieval", features = ["testkit"] }`（若已间接存在则补 features）。

`crates/gateway/src/lib.rs` 的 `mod tests` 加：
```rust
    #[tokio::test]
    async fn with_embedder_rebuild_builds_vector_snapshot_no_upstreams() {
        let state = GatewayState::with_embedder(
            "vector",
            std::sync::Arc::new(retrieval::MockEmbedder::new(16)),
        )
        .expect("vector state");
        // No upstreams -> empty catalog; rebuild must succeed (embed of [] is fine).
        state.rebuild_snapshot().await.expect("rebuild ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }
```
> 若 gateway dev-deps 缺 `metatools`，加 `metatools = { path = "../metatools" }`。

- [ ] **Step 6: 全量构建 + 测试**

Run: `cargo build --workspace && cargo test --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all --check`
Expected: 全 PASS / 干净（含 retrieval testkit 门控测试）。

- [ ] **Step 7: 文档（L4 gateway + L3 gateway）**

- `docs/L4-api/gateway-lib.md`：更新 `build_strategy` 新签名引用；新增 `GatewayState::with_embedder`；rebuild 用 `self.embedder`。
- `docs/L3-details/gateway.md`：补「策略工厂按 name+embedder 构建；vector 需 embedder，缺则 `EmbedderRequired`；rebuild 复用 state 持有的 embedder（带缓存，跨 rebuild）」。

- [ ] **Step 8: 提交**

```bash
git add crates/retrieval/src/lib.rs crates/retrieval/tests/vector.rs crates/gateway crates/mcpgw/src/main.rs docs/L4-api/gateway-lib.md docs/L3-details/gateway.md
git commit -m "feat(gateway): inject Embedder; build_strategy(name, embedder) for vector (M2-A T5)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6: `embedder` crate —— `OpenAiEmbedder`（reqwest，OpenAI-兼容）

新建独立 crate，HTTP/序列化只在此。实现 `retrieval::Embedder`，调 `POST {base_url}/embeddings`。用本地 axum stub 做 mock-HTTP 单测（无需真实 key）。

**Files:**
- Create: `crates/embedder/Cargo.toml`, `crates/embedder/src/lib.rs`, `crates/embedder/tests/openai.rs`
- Modify: `Cargo.toml`(workspace `members`)

- [ ] **Step 1: 建 crate + 注册 member**

`Cargo.toml`(workspace) 的 `members` 数组追加 `"crates/embedder"`。

`crates/embedder/Cargo.toml`（新建）：
```toml
[package]
name = "embedder"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
retrieval = { path = "../retrieval" }
async-trait = { workspace = true }
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls-tls"] }
serde = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["full"] }
axum = { workspace = true }
```
> reqwest 用 0.13（与 rmcp 解析版本一致，复用编译产物）；若解析冲突用 `cargo add -p embedder reqwest@0.13 --no-default-features -F json,rustls-tls`。

- [ ] **Step 2: 写失败测试（mock-HTTP）**

`crates/embedder/tests/openai.rs`（新建）：
```rust
use std::sync::{Arc, Mutex};

use axum::{extract::State, routing::post, Json, Router};
use embedder::OpenAiEmbedder;
use retrieval::Embedder;
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<Value>>>;

async fn embeddings_stub(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
    // Return a 3-dim embedding per input, with index, intentionally OUT of order to
    // verify the client sorts by `index`.
    let inputs = body["input"].as_array().cloned().unwrap_or_default();
    let mut data: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, _)| json!({"object":"embedding","index": i, "embedding":[i as f32, 0.0, 1.0]}))
        .collect();
    data.reverse();
    Json(json!({"object":"list","data": data, "model":"stub"}))
}

async fn spawn_stub() -> (String, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/embeddings", post(embeddings_stub))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

#[tokio::test]
async fn embeds_via_openai_compatible_endpoint() {
    let (base, seen) = spawn_stub().await;
    let e = OpenAiEmbedder::new(base, "text-embedding-3-small".into(), "sk-test".into(), Some(3), None);

    let out = e.embed(&["alpha".into(), "beta".into()]).await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].len(), 3);
    // index-sorted: input 0 -> [0,0,1], input 1 -> [1,0,1]
    assert_eq!(out[0], vec![0.0, 0.0, 1.0]);
    assert_eq!(out[1], vec![1.0, 0.0, 1.0]);

    // request body carried model + input[].
    let body = seen.lock().unwrap()[0].clone();
    assert_eq!(body["model"], "text-embedding-3-small");
    assert_eq!(body["input"], json!(["alpha", "beta"]));
}

#[tokio::test]
async fn dimension_mismatch_is_error() {
    let (base, _) = spawn_stub().await;
    // stub returns dim 3; configure expected dim 99 -> Dimension error.
    let e = OpenAiEmbedder::new(base, "m".into(), "sk".into(), Some(99), None);
    assert!(matches!(
        e.embed(&["x".into()]).await,
        Err(retrieval::EmbedError::Dimension { .. })
    ));
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p embedder`
Expected: 编译失败（`OpenAiEmbedder` 未定义）。

- [ ] **Step 4: 实现 `embedder/src/lib.rs`**

```rust
//! `OpenAiEmbedder`: an `Embedder` backed by an OpenAI-compatible `/embeddings` endpoint
//! (OpenAI, or local servers like Ollama/LM Studio/vLLM that speak the same shape). The only
//! crate in the workspace that depends on reqwest; everything else uses the `Embedder` trait.

use std::time::Duration;

use async_trait::async_trait;
use retrieval::{EmbedError, Embedder};
use serde::Deserialize;

#[derive(Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

/// Calls `POST {base_url}/embeddings` with a Bearer token. `dim`, when set, is enforced.
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    dim: Option<usize>,
}

impl OpenAiEmbedder {
    pub fn new(
        base_url: String,
        model: String,
        api_key: String,
        dim: Option<usize>,
        timeout: Option<Duration>,
    ) -> Self {
        let mut builder = reqwest::Client::builder();
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        let client = builder.build().expect("reqwest client builds");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
            dim,
        }
    }
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let url = format!("{}/embeddings", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "model": self.model, "input": texts }))
            .send()
            .await
            .map_err(|e| EmbedError::Provider(format!("request failed: {e}")))?;
        if !resp.status().is_success() {
            let code = resp.status();
            return Err(EmbedError::Provider(format!("HTTP {code} from embeddings endpoint")));
        }
        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| EmbedError::Provider(format!("decode failed: {e}")))?;

        // Sort by `index` so output order matches input order regardless of server ordering.
        let mut data = parsed.data;
        data.sort_by_key(|d| d.index);
        if data.len() != texts.len() {
            return Err(EmbedError::Provider(format!(
                "expected {} embeddings, got {}",
                texts.len(),
                data.len()
            )));
        }
        if let Some(expected) = self.dim {
            for d in &data {
                if d.embedding.len() != expected {
                    return Err(EmbedError::Dimension {
                        expected,
                        got: d.embedding.len(),
                    });
                }
            }
        }
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }

    fn dim(&self) -> usize {
        self.dim.unwrap_or(0)
    }
}
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p embedder`
Expected: 2 个测试 PASS。

- [ ] **Step 6: 文档（L4 + L2 embedder）**

- 新建 `docs/L4-api/embedder-openai.md`：`OpenAiEmbedder::new` 签名、请求/响应形状、index 排序、dim 校验、错误→`EmbedError`。
- 新建 `docs/L2-components/embedder.md`：crate 职责（唯一 HTTP 依赖，实现 `retrieval::Embedder`）。

- [ ] **Step 7: 提交**

```bash
git add crates/embedder Cargo.toml docs/L4-api/embedder-openai.md docs/L2-components/embedder.md
git commit -m "feat(embedder): OpenAI-compatible OpenAiEmbedder (M2-A T6)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 7: config `[retrieval.vector]` + mcpgw 启动期建 embedder 并注入

加配置段与校验；mcpgw 按 config 构建 `OpenAiEmbedder → CachingEmbedder`（启动期 fail-fast 读 key）并注入 `GatewayState`。

**Files:**
- Modify: `crates/config/src/lib.rs`
- Modify: `crates/mcpgw/src/main.rs`, `crates/mcpgw/Cargo.toml`

- [ ] **Step 1: 写失败测试（config）**

`crates/config/src/lib.rs` 的 `mod tests` 末尾加：
```rust
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
    fn vector_section_rejects_unknown_field() {
        let err = Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"K\"\nbogus=1\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p config parses_retrieval_vector_section`
Expected: 编译失败（`RetrievalConfig.vector` / `VectorConfig` 未定义）。

- [ ] **Step 3: 实现 config 类型 + 校验**

`crates/config/src/lib.rs`：
- `RetrievalConfig` 加字段 + 默认：
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    pub strategy: String,
    pub top_k: usize,
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
```
- `validate()` 在 strategy 检查之后加：
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
                            "[retrieval.vector] base_url/model/api_key_env must be non-empty".into(),
                        ));
                    }
                }
            }
        }
```

- [ ] **Step 4: 运行 config 测试，确认通过**

Run: `cargo test -p config`
Expected: 3 个新测试 + 原测试全 PASS。

- [ ] **Step 5: 写失败测试（mcpgw build_embedder）**

`crates/mcpgw/Cargo.toml` `[dependencies]` 加：
```toml
embedder = { path = "../embedder" }
retrieval = { path = "../retrieval" }
```
（`retrieval` 若已存在则跳过。）

`crates/mcpgw/src/main.rs` 的 `mod tests` 加：
```rust
    #[test]
    fn build_embedder_none_for_bm25() {
        let cfg = config::Config::default_from_empty();
        assert!(build_embedder(&cfg).unwrap().is_none());
    }

    #[test]
    fn build_embedder_fails_fast_on_missing_key() {
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_NO_KEY\"\n",
        )
        .unwrap();
        assert!(build_embedder(&cfg).is_err());
    }

    #[test]
    fn build_embedder_some_for_vector_with_key() {
        std::env::set_var("MCPGW_M2_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_KEY\"\n",
        )
        .unwrap();
        assert!(build_embedder(&cfg).unwrap().is_some());
    }
```

- [ ] **Step 6: 实现 mcpgw `build_embedder` + 注入 `prepare_state`**

`crates/mcpgw/src/main.rs`：
- 在 `prepare_state` 之前加：
```rust
/// Build the retrieval embedder from config (vector/hybrid). Returns `None` for bm25.
/// Reads the API key from its env var (fail-fast) and wraps the provider in a content-hash
/// cache shared across snapshot rebuilds.
fn build_embedder(cfg: &config::Config) -> Result<Option<Arc<dyn retrieval::Embedder>>, String> {
    match cfg.retrieval.strategy.as_str() {
        "vector" => {
            let v = cfg
                .retrieval
                .vector
                .as_ref()
                .ok_or("strategy=\"vector\" requires [retrieval.vector]")?;
            let api_key = std::env::var(&v.api_key_env)
                .map_err(|_| format!("[retrieval.vector]: env {:?} is not set", v.api_key_env))?;
            let openai = embedder::OpenAiEmbedder::new(
                v.base_url.clone(),
                v.model.clone(),
                api_key,
                v.dim,
                v.timeout_ms.map(std::time::Duration::from_millis),
            );
            Ok(Some(Arc::new(retrieval::CachingEmbedder::new(Arc::new(openai)))))
        }
        _ => Ok(None),
    }
}
```
- `prepare_state` 中把 `GatewayState::new(...)` 替换为：
```rust
    let state = match build_embedder(cfg)? {
        Some(embedder) => Arc::new(
            gateway::GatewayState::with_embedder(&cfg.retrieval.strategy, embedder)
                .map_err(|e| e.to_string())?,
        ),
        None => Arc::new(
            gateway::GatewayState::new(&cfg.retrieval.strategy).map_err(|e| e.to_string())?,
        ),
    };
```

- [ ] **Step 7: 全量构建 + 测试**

Run: `cargo build --workspace && cargo test --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all --check`
Expected: 全 PASS / 干净。

- [ ] **Step 8: 文档（L4 config + L4 mcpgw + L2 config）**

- `docs/L4-api/config-lib.md`：新增 `RetrievalConfig.vector` / `VectorConfig` 字段；vector 校验。
- `docs/L4-api/mcpgw-main.md`：新增 `build_embedder`；`prepare_state` 注入路径。
- `docs/L2-components/config.md`：补 `[retrieval.vector]` 段。

- [ ] **Step 9: 提交**

```bash
git add crates/config/src/lib.rs crates/mcpgw docs/L4-api/config-lib.md docs/L4-api/mcpgw-main.md docs/L2-components/config.md
git commit -m "feat(config,mcpgw): [retrieval.vector] + startup embedder wiring (M2-A T7)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 8: 验证脚本 + 门控真实向量冒烟

提供一个 stdlib-only 的 python 验证脚本（先打真实 embeddings API、人工核对余弦排序），以及一个 `#[ignore]` 的真实向量冒烟测试（语义查询命中 BM25 会漏的工具，证明向量增益）。

**Files:**
- Create: `scripts/embed_check.py`
- Create: `crates/mcpgw/tests/smoke_vector_real.rs`
- Modify: `crates/mcpgw/Cargo.toml`（dev `catalog`、若缺）

- [ ] **Step 1: 验证脚本**

`scripts/embed_check.py`（新建，仅用标准库；先验证真实端点与余弦排序）：
```python
#!/usr/bin/env python3
"""Sanity-check an OpenAI-compatible /embeddings endpoint against the tool catalog.

Reads env: OPENAI_API_KEY (required), MCPGW_EMBED_BASE_URL (default OpenAI),
MCPGW_EMBED_MODEL (default text-embedding-3-small). Prints cosine ranking of tools for a
few semantic queries — manual inspection step before trusting the Rust integration.
"""
import json, math, os, sys, urllib.request

BASE = os.environ.get("MCPGW_EMBED_BASE_URL", "https://api.openai.com/v1").rstrip("/")
MODEL = os.environ.get("MCPGW_EMBED_MODEL", "text-embedding-3-small")
KEY = os.environ.get("OPENAI_API_KEY")
if not KEY:
    sys.exit("set OPENAI_API_KEY (and optionally MCPGW_EMBED_BASE_URL / MCPGW_EMBED_MODEL)")

TOOLS = {
    "slack__post_message": "Send a chat message to a Slack channel",
    "weather__get_forecast": "Get the weather forecast for a location",
    "github__create_issue": "Create a new issue in a GitHub repository",
    "filesystem__write_file": "Write contents to a file on disk",
}
QUERIES = ["communicate with my team", "will it rain tomorrow", "report a bug"]

def embed(texts):
    body = json.dumps({"model": MODEL, "input": texts}).encode()
    req = urllib.request.Request(
        f"{BASE}/embeddings", data=body,
        headers={"Authorization": f"Bearer {KEY}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req) as r:
        data = json.load(r)["data"]
    return [d["embedding"] for d in sorted(data, key=lambda d: d["index"])]

def cos(a, b):
    dot = sum(x*y for x, y in zip(a, b))
    na = math.sqrt(sum(x*x for x in a)); nb = math.sqrt(sum(y*y for y in b))
    return dot / (na*nb)

names = list(TOOLS)
tvecs = embed([TOOLS[n] for n in names])
for q, qv in zip(QUERIES, embed(QUERIES)):
    ranked = sorted(((cos(qv, tv), n) for n, tv in zip(names, tvecs)), reverse=True)
    print(f"\nQUERY: {q!r}")
    for score, n in ranked:
        print(f"  {score:.3f}  {n}")
```

- [ ] **Step 2: 跑脚本（有 key 时手动验证；无 key 跳过）**

Run（有真实 key 时）: `OPENAI_API_KEY=sk-... python scripts/embed_check.py`
Expected: 每个语义 query 的 top-1 与直觉一致（如 "communicate with my team" → `slack__post_message`）。无 key 则跳过本步（脚本会提示并退出）。

- [ ] **Step 3: 门控真实冒烟测试**

`crates/mcpgw/Cargo.toml` `[dev-dependencies]` 确保有 `catalog = { path = "../catalog" }`、`retrieval = { path = "../retrieval" }`、`embedder = { path = "../embedder" }`（缺则补）。

`crates/mcpgw/tests/smoke_vector_real.rs`（新建）：
```rust
//! Gated real vector smoke: embed the tool catalog with a real OpenAI-compatible endpoint and
//! assert a *semantic* query (no shared literal tokens) ranks the right tool first — something
//! BM25 cannot do. #[ignore]d; needs OPENAI_API_KEY (+ optional MCPGW_EMBED_BASE_URL / _MODEL).
//!
//! Run: cargo test -p mcpgw --test smoke_vector_real -- --ignored --nocapture

use std::sync::Arc;

use catalog::{Catalog, ToolDef};
use embedder::OpenAiEmbedder;
use retrieval::{RetrievalStrategy, VectorStrategy};
use serde_json::Value;

fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
    ToolDef { server: server.into(), name: name.into(), description: desc.into(), input_schema: Value::Null }
}

#[tokio::test]
#[ignore = "real embeddings: needs OPENAI_API_KEY (+ optional MCPGW_EMBED_BASE_URL/_MODEL)"]
async fn semantic_query_ranks_right_tool_first() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("skipping: OPENAI_API_KEY not set");
        return;
    };
    let base = std::env::var("MCPGW_EMBED_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
    let model = std::env::var("MCPGW_EMBED_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    let catalog = Catalog::from_tooldefs(vec![
        tool("slack", "post_message", "Send a chat message to a Slack channel"),
        tool("weather", "get_forecast", "Get the weather forecast for a location"),
        tool("github", "create_issue", "Create a new issue in a GitHub repository"),
        tool("filesystem", "write_file", "Write contents to a file on disk"),
    ]);
    let embedder = Arc::new(OpenAiEmbedder::new(base, model, key, None, None));
    let mut strat = VectorStrategy::new(embedder);
    strat.index(&catalog).await;

    // "communicate with my team" shares no literal token with any tool description, so BM25
    // would return nothing; vector retrieval should still rank Slack first.
    let hits = strat.search("communicate with my team", 4).await;
    assert_eq!(hits.first().map(|h| h.qualified_name.as_str()), Some("slack__post_message"),
        "semantic top-1 should be slack__post_message, got: {hits:?}");
}
```

- [ ] **Step 4: 编译门控测试（不运行真实）**

Run: `cargo test -p mcpgw --test smoke_vector_real --no-run`
Expected: 编译通过；普通 `cargo test` 因 `#[ignore]` 跳过实际运行。有 key 时可 `-- --ignored` 手动验证。

- [ ] **Step 5: 文档（L3 mcpgw-cli）**

`docs/L3-details/mcpgw-cli.md`：补「向量检索验证脚本 `scripts/embed_check.py`（stdlib-only）+ 门控真实冒烟 `smoke_vector_real`（语义查询，需 OPENAI_API_KEY）」。

- [ ] **Step 6: 提交**

```bash
git add scripts/embed_check.py crates/mcpgw/Cargo.toml crates/mcpgw/tests/smoke_vector_real.rs docs/L3-details/mcpgw-cli.md
git commit -m "test(mcpgw): vector validation script + gated real semantic smoke (M2-A T8)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 9: L1/L2 文档收口 + 全量验证

**Files:** Modify `docs/L1-overview.md`, `docs/L2-components/retrieval.md`, `docs/README.md`

- [ ] **Step 1: L1 总览**

`docs/L1-overview.md`：检索从"仅 BM25 同步"→"可插拔、异步（`async-trait`）、失败降级；BM25 / Vector(云嵌入余弦+BM25降级)"；架构图加 `embedder` crate 与向量路径；更新 crate 列表（新增 `embedder`）与测试总数（运行 `cargo test --all-features` 累加填入）。

- [ ] **Step 2: L2 retrieval 收口**

`docs/L2-components/retrieval.md`：补 `RetrievalStrategy`(async)、`VectorStrategy`、`Embedder`/`CachingEmbedder` 职责与协作（确保与 T2–T5 增量不重复、不矛盾）。

- [ ] **Step 3: README 清单**

`docs/README.md`：L4 清单加 `retrieval-embedder.md`、`retrieval-vector.md`、`embedder-openai.md`；若有 crate 清单，加 `embedder`。

- [ ] **Step 4: 跨层一致性校对**

逐项核对 spec §1–§8 都有对应任务/代码点；L1↔L2↔L3↔L4 对 async/降级/缓存/默认仍 bm25/向量配置的描述一致。修正漂移。

- [ ] **Step 5: 全量验证**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净、clippy 无告警、所有测试 PASS（含 retrieval testkit 门控；`#[ignore]` 的真实冒烟被跳过）。

- [ ] **Step 6: 提交**

```bash
git add docs/L1-overview.md docs/L2-components/retrieval.md docs/README.md
git commit -m "docs: L1/L2 sync for M2-A vector retrieval (M2-A T9)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 收尾（全部任务完成后）

1. 派发最终整体 code review（spec 覆盖、async 正确性、降级、密钥/缓存、无回归）。
2. 处理 blocking 项（如有）。
3. 用 superpowers:finishing-a-development-branch 把 `feat/m2a-vector-retrieval` 合回 master（`--no-ff`，本地），删分支，更新 roadmap（M2-A done；M2-B=Hybrid+默认切换 待办）。

## 实现期需现场确认/可能回退的点（spec §10）
- `async-trait` 对 `Box<dyn RetrievalStrategy>` 的 Send 约束（search future 跨 await 持 `Arc<GatewaySnapshot>`）——T1 编译实证。
- reqwest 版本与既有依赖的统一（rmcp 解析 0.13.x；downstream dev 用 0.12）——embedder 取 0.13，必要时 `cargo add` 解析。
- `MockEmbedder` 伪向量是否足以让"相关>无关"余弦断言稳定（token 分桶可能在小 dim 下哈希碰撞）——T2/T4 用足够 dim（64/128）规避；若偶发碰撞，增大 dim 或换 query。
- OpenAI 响应 `data[].index` 排序假设——T6 stub 故意乱序验证。
