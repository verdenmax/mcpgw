# M2.T5：SubagentStrategy（BM25 预筛 → 小模型重排）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增可选 `"subagent"` 检索策略：BM25 预筛出候选 shortlist，再用便宜的小模型（OpenAI 兼容 `/chat/completions`）在候选里重排 top-k；失败透明降级回 BM25。opt-in，**默认仍 bm25**。

**Architecture:** `retrieval`（无 HTTP）新增通用 `ChatModel` 抽象 + `MockChatModel`（testkit）+ `SubagentStrategy`（内置 `Bm25Strategy` 预筛 + prompt 构造/解析/降级）。后端注入重构为 `Backends { embedder, chat }`，`build_strategy(name, &Backends)`。新 `chat` crate 放真实 HTTP 实现 `OpenAiChat`（与 `embedder` crate 对称，retrieval 仍无 HTTP）。`GatewayState` 持有 `Backends`，`mcpgw::build_backends` 按 strategy 建后端。

**Tech Stack:** Rust 2021 / `async-trait` / reqwest 0.13（仅 `chat` crate）/ tokio / serde / 复用 `Bm25Strategy`。

**Spec:** `docs/superpowers/specs/2026-06-14-mcpgw-m2t5-subagent-retrieval-design.md`

---

## 已确认的关键事实（实现时照用）

- `RetrievalStrategy` 是 async（`#[async_trait]`）：`async fn index(&mut self, &Catalog)` / `async fn search(&self, &str, usize) -> Vec<ScoredTool>`。
- `Bm25Strategy::search` 带 `score>0` 过滤，**只返回命中词项的文档**，每个 `ScoredTool` 含 `qualified_name` + `description`（足够构造 prompt，无需另存目录）。
- 当前 `crates/retrieval/src/lib.rs` 模块头（M2-B 后，fmt 干净）：
```
mod caching;
mod embedder;
mod hybrid;
mod vector;
pub use caching::CachingEmbedder;
#[cfg(feature = "testkit")]
pub use embedder::MockEmbedder;
pub use embedder::{EmbedError, Embedder};
pub use hybrid::HybridStrategy;
pub use vector::VectorStrategy;
```
- 当前 `build_strategy(name: &str, embedder: Option<&std::sync::Arc<dyn Embedder>>)`，臂：bm25 / vector（需 embedder）/ hybrid（需 embedder）/ other → NotImplemented。`StrategyError { NotImplemented, EmbedderRequired }`。
- `GatewayState` 现持有 `embedder: Option<Arc<dyn Embedder>>`；`build(strategy, Option<Arc<dyn Embedder>>)`、`new`、`with_embedder`、`rebuild_snapshot` 均经 `build_strategy(name, embedder.as_ref())`。gateway dev-dep 已含 `retrieval` 的 `testkit`。
- `mcpgw::build_embedder(cfg) -> Result<Option<Arc<dyn retrieval::Embedder>>, String>`：仅 `"vector"|"hybrid"` 建 embedder，其它 None；`prepare_state` 据此走 `with_embedder` 或 `new`。
- `embedder` crate 是范本：`OpenAiEmbedder`（reqwest 0.13 + rustls，POST `{base_url}/embeddings`，bearer，按 index 排序，非 2xx 附 ≤500 字 body 片段，空输入短路）。`chat` crate 与之对称。
- `config::validate`：`KNOWN=["bm25","vector","hybrid"]`；`matches!(strategy, "vector"|"hybrid")` 时要求 `[retrieval.vector]`。`[retrieval.vector]` 用 `serde(deny_unknown_fields)`。
- `retrieval` 的 testkit 集成测试放 `crates/retrieval/tests/<name>.rs` + `Cargo.toml` `[[test]] required-features=["testkit"]`。
- **本仓库强制 `cargo fmt --all --check` 门禁**；每个 task 提交前先 `cargo fmt -p <crate>` 并确认 `--check` 干净。
- 分层文档（L1–L4 + README + roadmap）是每个 task 的 DoD。

## File Structure

| 文件 | 职责 | 任务 |
|------|------|------|
| `crates/retrieval/src/chat.rs`（新） | `ChatModel` trait + `ChatError` + `MockChatModel`(testkit) | T1 |
| `crates/retrieval/src/lib.rs` | `mod chat;` + re-export；`Backends` 结构体；`build_strategy(name,&Backends)`；`StrategyError::ChatModelRequired`；subagent 臂；更新工厂测试 | T1/T2/T3 |
| `crates/retrieval/src/subagent.rs`（新） | `SubagentStrategy`（预筛 + prompt + 解析 + 重排 + 降级）+ 私有解析单测 | T3 |
| `crates/retrieval/tests/subagent.rs`（新） | testkit 集成测试（重排/幻觉过滤/降级/空/解析失败/工厂） | T4 |
| `crates/retrieval/Cargo.toml` | 加 `[[test]] name="subagent" required-features=["testkit"]` | T4 |
| `crates/chat/*`（新 crate） | `OpenAiChat: ChatModel`（reqwest /chat/completions）+ 测试 | T5 |
| `Cargo.toml`(workspace) | members 加 `crates/chat` | T5 |
| `crates/config/src/lib.rs` | `[retrieval.subagent]` 结构 + validate + `KNOWN` 加 subagent + 测试 | T6 |
| `crates/gateway/src/lib.rs` | `GatewayState` 持 `Backends`；`with_backends`（`with_embedder` 留薄封装）；rebuild 传 `&Backends`；subagent 快照测试 | T2/T7 |
| `crates/mcpgw/src/main.rs` | `build_embedder` → `build_backends`（subagent 建 chat）；`prepare_state` 用 `with_backends`；测试 | T2/T7 |
| `docs/L1`–`L4` / `README` / roadmap | 分层文档（DoD） | T8 |

## 前置：建分支

```bash
git switch -c feat/m2t5-subagent-retrieval
```
（spec 已在 master；实现在该分支，最后 `--no-ff` 合并。）

---

<!-- TASKS APPENDED INCREMENTALLY BELOW -->

### Task 1: `ChatModel` 抽象 + `MockChatModel`（testkit）

**Files:**
- Create: `crates/retrieval/src/chat.rs`
- Modify: `crates/retrieval/src/lib.rs`（`mod chat;` + re-export）

纯新增抽象（镜像 `embedder.rs` 的 `Embedder`/`MockEmbedder` 模式）。

- [ ] **Step 1: 创建 `crates/retrieval/src/chat.rs`**

```rust
//! The `ChatModel` abstraction: a single-shot chat completion (system + user -> text). The
//! HTTP-backed provider lives in the separate `chat` crate; this module only defines the trait,
//! errors, and a deterministic `MockChatModel` (behind the `testkit` feature) for tests.

use async_trait::async_trait;

/// Errors from a chat completion. Provider-agnostic so `retrieval` needs no HTTP dependency.
#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("chat provider error: {0}")]
    Provider(String),
    #[error("chat model returned no usable content")]
    Empty,
}

/// A single-shot chat model: a system + user prompt yields assistant text.
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError>;
}

#[cfg(feature = "testkit")]
mod mock {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Deterministic test chat model: returns a scripted `reply` (or errors when `fail`), and
    /// records call count + the last (system, user) prompts for assertions.
    pub struct MockChatModel {
        reply: String,
        fail: bool,
        pub calls: Arc<AtomicUsize>,
        pub last_system: Arc<Mutex<String>>,
        pub last_user: Arc<Mutex<String>>,
    }

    impl MockChatModel {
        /// A model that always returns `reply`.
        pub fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                fail: false,
                calls: Arc::new(AtomicUsize::new(0)),
                last_system: Arc::new(Mutex::new(String::new())),
                last_user: Arc::new(Mutex::new(String::new())),
            }
        }
        /// A model whose `complete` always errors (drives degradation tests).
        pub fn failing() -> Self {
            Self {
                fail: true,
                ..Self::new("")
            }
        }
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_system.lock().unwrap() = system.to_string();
            *self.last_user.lock().unwrap() = user.to_string();
            if self.fail {
                return Err(ChatError::Provider("mock failure".into()));
            }
            Ok(self.reply.clone())
        }
    }
}

#[cfg(feature = "testkit")]
pub use mock::MockChatModel;

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn mock_returns_scripted_reply_and_records_prompts() {
        let m = MockChatModel::new("hello");
        let out = m.complete("sys", "usr").await.expect("ok");
        assert_eq!(out, "hello");
        assert_eq!(m.calls.load(Ordering::SeqCst), 1);
        assert_eq!(*m.last_system.lock().unwrap(), "sys");
        assert_eq!(*m.last_user.lock().unwrap(), "usr");
    }

    #[tokio::test]
    async fn failing_mock_errors() {
        let m = MockChatModel::failing();
        assert!(m.complete("s", "u").await.is_err());
    }
}
```

- [ ] **Step 2: 在 `crates/retrieval/src/lib.rs` 模块头接线**

把现有模块头：
```rust
mod caching;
mod embedder;
mod hybrid;
mod vector;
pub use caching::CachingEmbedder;
#[cfg(feature = "testkit")]
pub use embedder::MockEmbedder;
pub use embedder::{EmbedError, Embedder};
pub use hybrid::HybridStrategy;
pub use vector::VectorStrategy;
```
改为（新增 `mod chat;` 与 chat 的 re-export，保持模块名有序）：
```rust
mod caching;
mod chat;
mod embedder;
mod hybrid;
mod vector;
pub use caching::CachingEmbedder;
pub use chat::{ChatError, ChatModel};
#[cfg(feature = "testkit")]
pub use chat::MockChatModel;
#[cfg(feature = "testkit")]
pub use embedder::MockEmbedder;
pub use embedder::{EmbedError, Embedder};
pub use hybrid::HybridStrategy;
pub use vector::VectorStrategy;
```

- [ ] **Step 3: 运行测试（testkit）确认通过**

Run: `cargo test -p retrieval --features testkit chat::`
Expected: `mock_returns_scripted_reply_and_records_prompts` 与 `failing_mock_errors` PASS；整个 crate 仍编译。

- [ ] **Step 4: fmt + clippy**

Run:
```bash
cargo fmt -p retrieval
cargo fmt -p retrieval -- --check     # 干净
cargo clippy -p retrieval --all-targets --all-features   # 无新告警
```

- [ ] **Step 5: 提交**

```bash
git add crates/retrieval/src/chat.rs crates/retrieval/src/lib.rs
git commit -m "feat(retrieval): ChatModel abstraction + MockChatModel (M2.T5 T1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

**Discipline:** 只加抽象与 mock，不碰 build_strategy/Backends（T2）。

---

### Task 2: `Backends` 注入重构（`build_strategy(name, &Backends)`）

破坏性签名变更——一次性更新 `retrieval` 与 `gateway` 全调用点，保持编译+测试绿。`mcpgw` 因保留 `with_embedder` 薄封装，本 task **不需改动**（留 T7）。subagent 臂留到 T3（此处 `"subagent"` 仍走 `NotImplemented`）。

**Files:**
- Modify: `crates/retrieval/src/lib.rs`、`crates/retrieval/tests/vector.rs`、`crates/retrieval/tests/hybrid.rs`
- Modify: `crates/gateway/src/lib.rs`

- [ ] **Step 1: retrieval —— `Backends` + `ChatModelRequired` + 新签名**

(a) 在 `crates/retrieval/src/lib.rs` 的 `StrategyError` 增加变体：
```rust
#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("retrieval strategy {0:?} is not implemented in this version")]
    NotImplemented(String),
    #[error("retrieval strategy {0:?} requires an embedder but none was configured")]
    EmbedderRequired(String),
    #[error("retrieval strategy {0:?} requires a chat model but none was configured")]
    ChatModelRequired(String),
}
```

(b) 紧接 `StrategyError` 之后、`build_strategy` 之前，新增 `Backends`：
```rust
/// Optional retrieval backends injected into `build_strategy`. Bundling them keeps the factory
/// signature stable as new backends are added. `bm25` needs none; `vector`/`hybrid` need
/// `embedder`; `subagent` needs `chat`.
#[derive(Default, Clone)]
pub struct Backends {
    pub embedder: Option<std::sync::Arc<dyn Embedder>>,
    pub chat: Option<std::sync::Arc<dyn ChatModel>>,
    /// Shortlist size for "subagent"'s BM25 prefilter (None -> default). Consumed by the
    /// subagent arm added in T3; carried here so the factory signature stays stable.
    pub subagent_candidates: Option<usize>,
}
```

(c) 把 `build_strategy` 改签名为 `(name, &Backends)`，臂改用 `backends.embedder`：
```rust
/// Construct a retrieval strategy by name, wired with the given optional `backends`.
///
/// Takes a plain `&str` (not a config type) so this crate stays free of any dependency on
/// `config` — callers pass `cfg.retrieval.strategy.as_str()`.
pub fn build_strategy(
    name: &str,
    backends: &Backends,
) -> Result<Box<dyn RetrievalStrategy>, StrategyError> {
    match name {
        "bm25" => Ok(Box::new(Bm25Strategy::new())),
        "vector" => match backends.embedder.as_ref() {
            Some(e) => Ok(Box::new(VectorStrategy::new(e.clone()))),
            None => Err(StrategyError::EmbedderRequired(name.to_string())),
        },
        "hybrid" => match backends.embedder.as_ref() {
            Some(e) => Ok(Box::new(HybridStrategy::new(e.clone()))),
            None => Err(StrategyError::EmbedderRequired(name.to_string())),
        },
        other => Err(StrategyError::NotImplemented(other.to_string())),
    }
}
```

(d) 更新 `lib.rs` 中的 `#[cfg(test)]` 工厂测试调用点：
- `build_strategy_returns_bm25_and_indexes`：`build_strategy("bm25", None)` → `build_strategy("bm25", &Backends::default())`。
- `build_strategy_errors_appropriately`：三处 `None` 参数都改为 `&Backends::default()`（断言变体不变：hybrid/vector → `EmbedderRequired`，nope → `NotImplemented`）。

- [ ] **Step 2: retrieval —— 更新两个 testkit 集成测试的调用点**

- `crates/retrieval/tests/vector.rs`：把 `use retrieval::{...}` 加上 `Backends`；把 `build_strategy("vector", Some(&embedder))` 改为
  `build_strategy("vector", &Backends { embedder: Some(embedder.clone()), ..Default::default() })`。
- `crates/retrieval/tests/hybrid.rs`：把 `use retrieval::{...}` 加上 `Backends`；把 `build_strategy("hybrid", Some(&embedder))` 改为
  `build_strategy("hybrid", &Backends { embedder: Some(embedder.clone()), ..Default::default() })`。

- [ ] **Step 3: gateway —— `GatewayState` 持有 `Backends`**

在 `crates/gateway/src/lib.rs`：
- import 改为 `use retrieval::{build_strategy, Backends, Embedder};`。
- 字段 `embedder: Option<Arc<dyn Embedder>>` → `backends: Backends`（更新其上方 doc 注释为「持有可选检索后端（embedder/chat），跨 rebuild 复用」）。
- `build` 改为收 `Backends` 并经 `&backends` 调用工厂：
```rust
    fn build(strategy_name: &str, backends: Backends) -> Result<Self, GatewayError> {
        let strat = build_strategy(strategy_name, &backends)
            .map_err(|e| GatewayError::Strategy(e.to_string()))?;
        let empty = Catalog::new();
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))),
            registry: UpstreamRegistry::new(),
            strategy_name: Arc::from(strategy_name),
            backends,
            rebuild_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn new(strategy_name: &str) -> Result<Self, GatewayError> {
        Self::build(strategy_name, Backends::default())
    }

    /// Create state whose retrieval strategy is backed by `embedder` (for "vector"/"hybrid").
    pub fn with_embedder(
        strategy_name: &str,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self, GatewayError> {
        Self::build(
            strategy_name,
            Backends {
                embedder: Some(embedder),
                ..Default::default()
            },
        )
    }

    /// Create state with arbitrary retrieval `backends` (e.g. a `chat` model for "subagent").
    pub fn with_backends(strategy_name: &str, backends: Backends) -> Result<Self, GatewayError> {
        Self::build(strategy_name, backends)
    }
```
- `rebuild_snapshot` 内的工厂调用：`build_strategy(&self.strategy_name, self.embedder.as_ref())` → `build_strategy(&self.strategy_name, &self.backends)`。

> 既有 gateway 测试用 `with_embedder("vector"/"hybrid", mock)`，签名不变，**无需改**。

- [ ] **Step 4: 编译 + 全测试绿**

Run:
```bash
cargo test -p retrieval --all-features
cargo test -p gateway --all-features
```
Expected: 全 PASS（既有 bm25/vector/hybrid 路径不回归；`build_strategy_*`、tests/vector、tests/hybrid、gateway vector/hybrid 快照均绿）。

- [ ] **Step 5: fmt + clippy**

```bash
cargo fmt -p retrieval -p gateway
cargo fmt -p retrieval -p gateway -- --check
cargo clippy -p retrieval -p gateway --all-targets --all-features
```

- [ ] **Step 6: 提交**

```bash
git add crates/retrieval/src/lib.rs crates/retrieval/tests/vector.rs crates/retrieval/tests/hybrid.rs crates/gateway/src/lib.rs
git commit -m "refactor(retrieval,gateway): Backends injection for build_strategy (M2.T5 T2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

**Discipline:** 纯重构 + 既有测试护栏；不加 subagent 逻辑（T3）。`StrategyError::ChatModelRequired` 此处定义、T3 使用。

---

### Task 3: `SubagentStrategy` 核心 + `build_strategy` subagent 臂

**Files:**
- Create: `crates/retrieval/src/subagent.rs`
- Modify: `crates/retrieval/src/lib.rs`（`mod subagent;` + re-export + subagent 臂 + 工厂错误测试）
- Modify: `crates/retrieval/Cargo.toml`（`serde_json` 从 dev-dep 提升为正式依赖）

- [ ] **Step 1: `Cargo.toml` —— serde_json 提为正式依赖**

`crates/retrieval/Cargo.toml`：在 `[dependencies]` 加 `serde_json = { workspace = true }`，并从 `[dev-dependencies]` 删除原有的 `serde_json` 行（现由正式依赖覆盖；解析 LLM JSON 回复是非测试代码）。`[dev-dependencies]` 保留 `tokio`。

- [ ] **Step 2: 创建 `crates/retrieval/src/subagent.rs`（解析函数先用 STUB）**

```rust
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
        s.push_str(&format!("{}. {}: {}\n", i + 1, t.qualified_name, t.description));
    }
    s
}

/// Parse the model reply into ordered qualified_names, keeping only names present in `allowed`
/// (drops hallucinations), de-duplicated, order-preserving. Empty on any failure (caller then
/// degrades to BM25).
fn parse_selection(reply: &str, allowed: &[ScoredTool]) -> Vec<String> {
    // STUB — replaced in Step 4.
    let _ = (reply, allowed);
    Vec::new()
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
            Ok(reply) => parse_selection(&reply, &shortlist),
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
}
```

- [ ] **Step 3: 运行解析单测，确认失败（STUB 返回空）**

Run: `cargo test -p retrieval subagent::tests`
Expected: `parse_keeps_only_allowed_in_order_dedup` FAIL（STUB 返回空，断言不符）；`parse_returns_empty_*` 恰好 PASS（STUB 也返回空）。

- [ ] **Step 4: 实现 `parse_selection`**

把 STUB 替换为：
```rust
fn parse_selection(reply: &str, allowed: &[ScoredTool]) -> Vec<String> {
    // Extract the first JSON array `[ ... ]` (models sometimes wrap it in prose / code fences).
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
```

- [ ] **Step 5: lib.rs 接线（mod + re-export + subagent 臂 + 工厂错误测试）**

(a) 模块头加 `mod subagent;`（在 `mod hybrid;` 与 `mod vector;` 之间，保持有序）与 re-export `pub use subagent::SubagentStrategy;`（紧跟 `pub use hybrid::HybridStrategy;`）。

(b) `build_strategy` 在 `"hybrid"` 臂之后、`other` 之前插入 subagent 臂：
```rust
        "subagent" => match backends.chat.as_ref() {
            Some(c) => Ok(Box::new(SubagentStrategy::new(
                c.clone(),
                backends
                    .subagent_candidates
                    .unwrap_or(subagent::DEFAULT_CANDIDATES),
            ))),
            None => Err(StrategyError::ChatModelRequired(name.to_string())),
        },
```

(c) 在 `build_strategy_errors_appropriately` 测试里追加一条断言：
```rust
        assert!(matches!(
            build_strategy("subagent", &Backends::default()),
            Err(StrategyError::ChatModelRequired(_))
        ));
```

- [ ] **Step 6: 运行，确认通过**

Run: `cargo test -p retrieval --all-features`
Expected: 全 PASS（`subagent::tests` 两条、`build_strategy_errors_appropriately`、既有 vector/hybrid/golden 等）。

- [ ] **Step 7: fmt + clippy**

```bash
cargo fmt -p retrieval
cargo fmt -p retrieval -- --check
cargo clippy -p retrieval --all-targets --all-features
```

- [ ] **Step 8: 提交**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/src/subagent.rs crates/retrieval/src/lib.rs
git commit -m "feat(retrieval): SubagentStrategy (BM25 prefilter + chat rerank + degrade) (M2.T5 T3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

**Discipline:** 解析/重排/降级是核心逻辑，严格 TDD（STUB→红→实现→绿）。集成（经 MockChatModel）留 T4。

---

### Task 4: subagent 集成测试（testkit，经 `MockChatModel`）

**Files:**
- Modify: `crates/retrieval/Cargo.toml`（加 `[[test]]`）
- Create: `crates/retrieval/tests/subagent.rs`

- [ ] **Step 1: 加 Cargo 测试目标**

`crates/retrieval/Cargo.toml` 末尾追加：
```toml
[[test]]
name = "subagent"
required-features = ["testkit"]
```

- [ ] **Step 2: 写集成测试**

创建 `crates/retrieval/tests/subagent.rs`：
```rust
//! SubagentStrategy integration tests over the deterministic MockChatModel.
use std::sync::atomic::Ordering;
use std::sync::Arc;

use catalog::{Catalog, ToolDef};
use retrieval::{
    build_strategy, Backends, Bm25Strategy, MockChatModel, RetrievalStrategy, SubagentStrategy,
};
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
async fn rerank_follows_model_order() {
    // BM25 shortlist for "create github issue" = the two github tools; the model reorders them.
    let mock = Arc::new(MockChatModel::new(
        r#"["github__list_pull_requests", "github__create_issue"]"#,
    ));
    let mut s = SubagentStrategy::new(mock.clone(), 20);
    s.index(&sample()).await;
    let hits: Vec<String> = s
        .search("create github issue", 5)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(
        hits,
        vec![
            "github__list_pull_requests".to_string(),
            "github__create_issue".to_string()
        ]
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    // The prompt carried the candidate list.
    assert!(mock.last_user.lock().unwrap().contains("Candidates:"));
}

#[tokio::test]
async fn hallucinated_names_are_dropped() {
    let mock = Arc::new(MockChatModel::new(
        r#"["nope__nonexistent", "github__create_issue"]"#,
    ));
    let mut s = SubagentStrategy::new(mock, 20);
    s.index(&sample()).await;
    let hits: Vec<String> = s
        .search("create github issue", 5)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(hits, vec!["github__create_issue".to_string()]);
}

#[tokio::test]
async fn degrades_to_bm25_when_chat_fails() {
    let mut s = SubagentStrategy::new(Arc::new(MockChatModel::failing()), 20);
    let cat = sample();
    s.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    let sq: Vec<String> = s
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    let bq: Vec<String> = b
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(sq, bq);
    assert!(!sq.is_empty());
}

#[tokio::test]
async fn garbage_reply_degrades_to_bm25() {
    let mut s = SubagentStrategy::new(Arc::new(MockChatModel::new("I think you want a tool")), 20);
    let cat = sample();
    s.index(&cat).await;
    let mut b = Bm25Strategy::new();
    b.index(&cat).await;
    let sq: Vec<String> = s
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    let bq: Vec<String> = b
        .search("repository", 3)
        .await
        .into_iter()
        .map(|h| h.qualified_name)
        .collect();
    assert_eq!(sq, bq);
}

#[tokio::test]
async fn empty_shortlist_returns_empty_without_calling_chat() {
    let mock = Arc::new(MockChatModel::new(r#"["github__create_issue"]"#));
    let mut s = SubagentStrategy::new(mock.clone(), 20);
    s.index(&sample()).await;
    // No lexical overlap -> BM25 shortlist empty -> no rerank, chat untouched.
    assert!(s.search("zzzznonexistent", 5).await.is_empty());
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn respects_top_k() {
    let mock = Arc::new(MockChatModel::new(
        r#"["github__create_issue", "github__list_pull_requests"]"#,
    ));
    let mut s = SubagentStrategy::new(mock, 20);
    s.index(&sample()).await;
    let hits = s.search("create github issue", 1).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].qualified_name, "github__create_issue");
}

#[tokio::test]
async fn build_strategy_subagent_with_chat_indexes_and_searches() {
    let backends = Backends {
        chat: Some(Arc::new(MockChatModel::new(r#"["weather__get_forecast"]"#))),
        ..Default::default()
    };
    let mut strat = build_strategy("subagent", &backends).expect("subagent ok with chat");
    strat.index(&sample()).await;
    let hits = strat.search("forecast", 5).await;
    assert_eq!(
        hits.first().map(|h| h.qualified_name.as_str()),
        Some("weather__get_forecast")
    );
}
```

- [ ] **Step 3: 运行，确认通过**

Run: `cargo test -p retrieval --features testkit --test subagent`
Expected: 7 个测试全部 PASS。若 `rerank_follows_model_order` 的 shortlist 与预期不符导致失败，**先核对** BM25 对 `"create github issue"` 的实际命中（应为两个 github 工具），不要弱化断言——必要时报告实际结果再调整 fixture/query。

- [ ] **Step 4: fmt + clippy**

```bash
cargo fmt -p retrieval
cargo fmt -p retrieval -- --check
cargo clippy -p retrieval --all-targets --all-features
```

- [ ] **Step 5: 提交**

```bash
git add crates/retrieval/Cargo.toml crates/retrieval/tests/subagent.rs
git commit -m "test(retrieval): subagent integration tests via MockChatModel (M2.T5 T4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: 新 `chat` crate（`OpenAiChat`，唯一第二个 HTTP 依赖）

镜像 `embedder` crate。`retrieval` 仍无 HTTP。

**Files:**
- Create: `crates/chat/Cargo.toml`、`crates/chat/src/lib.rs`、`crates/chat/tests/openai_chat.rs`
- Modify: `Cargo.toml`（workspace members 加 `crates/chat`）

- [ ] **Step 1: workspace 注册**

`Cargo.toml`（根）的 `members` 数组末尾加 `"crates/chat"`。

- [ ] **Step 2: `crates/chat/Cargo.toml`**

```toml
[package]
name = "chat"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
retrieval = { path = "../retrieval" }
async-trait = { workspace = true }
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls"] }
serde = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["full"] }
axum = { workspace = true }
```

- [ ] **Step 3: `crates/chat/src/lib.rs`**

```rust
//! `OpenAiChat`: a `ChatModel` backed by an OpenAI-compatible `/chat/completions` endpoint
//! (OpenAI, or local servers like Ollama/LM Studio/vLLM that speak the same shape). One of two
//! crates (with `embedder`) that depend on reqwest; everything else uses the `ChatModel` trait.

use std::time::Duration;

use async_trait::async_trait;
use retrieval::{ChatError, ChatModel};
use serde::Deserialize;

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

/// Calls `POST {base_url}/chat/completions` with a Bearer token and `temperature: 0` (stable
/// output). Returns `choices[0].message.content`.
pub struct OpenAiChat {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl OpenAiChat {
    pub fn new(
        base_url: String,
        model: String,
        api_key: String,
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
        }
    }
}

#[async_trait]
impl ChatModel for OpenAiChat {
    async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError> {
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "temperature": 0,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            }))
            .send()
            .await
            .map_err(|e| ChatError::Provider(format!("request failed: {e}")))?;
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(500).collect();
            return Err(ChatError::Provider(format!(
                "HTTP {code} from chat endpoint: {snippet}"
            )));
        }
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| ChatError::Provider(format!("decode failed: {e}")))?;
        match parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
        {
            Some(s) if !s.trim().is_empty() => Ok(s),
            _ => Err(ChatError::Empty),
        }
    }
}
```

- [ ] **Step 4: `crates/chat/tests/openai_chat.rs`**

```rust
use std::sync::{Arc, Mutex};

use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use chat::OpenAiChat;
use retrieval::{ChatError, ChatModel};
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<Value>>>;

async fn chat_stub(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
    Json(json!({"choices":[{"message":{"role":"assistant","content":"[\"a__b\"]"}}]}))
}

async fn spawn(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn returns_content_and_sends_expected_request() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(chat_stub))
        .with_state(seen.clone());
    let base = spawn(app).await;
    let c = OpenAiChat::new(base, "gpt-4o-mini".into(), "sk-x".into(), None);
    let out = c.complete("sys", "usr").await.expect("ok");
    assert_eq!(out, "[\"a__b\"]");
    let body = &seen.lock().unwrap()[0];
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "sys");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "usr");
}

#[tokio::test]
async fn non_2xx_is_provider_error() {
    async fn bad() -> (StatusCode, Json<Value>) {
        (StatusCode::BAD_REQUEST, Json(json!({"error":"bad model"})))
    }
    let base = spawn(Router::new().route("/chat/completions", post(bad))).await;
    let c = OpenAiChat::new(base, "m".into(), "k".into(), None);
    assert!(matches!(c.complete("s", "u").await, Err(ChatError::Provider(_))));
}

#[tokio::test]
async fn empty_choices_is_empty_error() {
    async fn empty() -> Json<Value> {
        Json(json!({"choices": []}))
    }
    let base = spawn(Router::new().route("/chat/completions", post(empty))).await;
    let c = OpenAiChat::new(base, "m".into(), "k".into(), None);
    assert!(matches!(c.complete("s", "u").await, Err(ChatError::Empty)));
}
```

- [ ] **Step 5: 运行 + fmt + clippy**

```bash
cargo test -p chat
cargo fmt -p chat
cargo fmt -p chat -- --check
cargo clippy -p chat --all-targets --all-features
```
Expected: 3 个测试 PASS；fmt 干净；clippy 无告警。

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml crates/chat/
git commit -m "feat(chat): OpenAiChat ChatModel over /chat/completions (M2.T5 T5)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 6: config `[retrieval.subagent]` + 校验

**Files:** Modify `crates/config/src/lib.rs`。

- [ ] **Step 1: 先加测试（红 / 编译失败）**

在 `tests` 模块新增（`cfg.retrieval.subagent` 字段尚不存在 → 先编译失败）：
```rust
    #[test]
    fn parses_retrieval_subagent_section() {
        let cfg = Config::from_toml_str(
            r#"
            [retrieval]
            strategy = "subagent"
            [retrieval.subagent]
            model = "gpt-4o-mini"
            api_key_env = "OPENAI_API_KEY"
            candidates = 30
            "#,
        )
        .unwrap();
        let s = cfg.retrieval.subagent.expect("subagent section");
        assert_eq!(s.base_url, "https://api.openai.com/v1"); // default
        assert_eq!(s.model, "gpt-4o-mini");
        assert_eq!(s.api_key_env, "OPENAI_API_KEY");
        assert_eq!(s.candidates, Some(30));
    }

    #[test]
    fn subagent_strategy_requires_subagent_section() {
        let err = Config::from_toml_str("[retrieval]\nstrategy = \"subagent\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn subagent_rejects_zero_candidates() {
        let err = Config::from_toml_str(
            "[retrieval]\nstrategy=\"subagent\"\n[retrieval.subagent]\nmodel=\"m\"\napi_key_env=\"K\"\ncandidates=0\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn subagent_section_rejects_unknown_field() {
        let err = Config::from_toml_str(
            "[retrieval]\nstrategy=\"subagent\"\n[retrieval.subagent]\nmodel=\"m\"\napi_key_env=\"K\"\nbogus=1\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p config subagent`
Expected: 编译失败（`subagent` 字段不存在）/ 断言失败。

- [ ] **Step 3: 实现 `SubagentConfig` + 字段 + KNOWN + validate**

(a) 在 `VectorConfig` 之后新增：
```rust
/// `[retrieval.subagent]`: OpenAI-compatible chat provider for the subagent reranker.
/// Secrets via env name only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentConfig {
    #[serde(default = "default_subagent_base_url")]
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// BM25 prefilter shortlist size handed to the model. None -> retrieval's default.
    #[serde(default)]
    pub candidates: Option<usize>,
}

fn default_subagent_base_url() -> String {
    "https://api.openai.com/v1".into()
}
```

(b) 在 `RetrievalConfig` 加字段（`vector` 之后）：
```rust
    /// `[retrieval.subagent]` provider config. Required when strategy is "subagent".
    pub subagent: Option<SubagentConfig>,
```
并在其 `Default` impl 里加 `subagent: None`。

(c) `validate` 里把 `KNOWN` 改为：
```rust
        const KNOWN: [&str; 4] = ["bm25", "vector", "hybrid", "subagent"];
```
并在 vector|hybrid 校验块之后追加 subagent 校验：
```rust
        if self.retrieval.strategy == "subagent" {
            match &self.retrieval.subagent {
                None => {
                    return Err(ConfigError::Invalid(
                        "strategy=\"subagent\" requires a [retrieval.subagent] section".into(),
                    ))
                }
                Some(s) => {
                    if s.base_url.trim().is_empty()
                        || s.model.trim().is_empty()
                        || s.api_key_env.trim().is_empty()
                    {
                        return Err(ConfigError::Invalid(
                            "[retrieval.subagent] base_url/model/api_key_env must be non-empty"
                                .into(),
                        ));
                    }
                    if s.candidates == Some(0) {
                        return Err(ConfigError::Invalid(
                            "[retrieval.subagent] candidates must be > 0".into(),
                        ));
                    }
                }
            }
        }
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p config`
Expected: 全 PASS（新增 4 条 + 既有，含 `rejects_unknown_strategy` 仍对未知名报错）。

- [ ] **Step 5: fmt + clippy + 提交**

```bash
cargo fmt -p config && cargo fmt -p config -- --check
cargo clippy -p config --all-targets
git add crates/config/src/lib.rs
git commit -m "feat(config): [retrieval.subagent] section + validation (M2.T5 T6)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 7: mcpgw `build_backends` 装配 + gateway subagent 快照测试

**Files:**
- Modify: `crates/mcpgw/Cargo.toml`（加 `chat` 依赖）、`crates/mcpgw/src/main.rs`
- Modify: `crates/gateway/src/lib.rs`（subagent 快照测试）

- [ ] **Step 1: mcpgw 依赖 chat crate**

`crates/mcpgw/Cargo.toml` `[dependencies]` 加 `chat = { path = "../chat" }`。

- [ ] **Step 2: `build_embedder` → `build_backends`**

把 `crates/mcpgw/src/main.rs` 的整个 `fn build_embedder` 替换为 `fn build_backends`：
```rust
/// Build the retrieval backends from config: an `embedder` for vector/hybrid, a `chat` model for
/// subagent, or nothing for bm25. Reads API keys from their env vars (fail-fast); the embedder is
/// wrapped in a content-hash cache shared across snapshot rebuilds.
fn build_backends(cfg: &config::Config) -> Result<retrieval::Backends, String> {
    let mut backends = retrieval::Backends::default();
    match cfg.retrieval.strategy.as_str() {
        "vector" | "hybrid" => {
            let v = cfg.retrieval.vector.as_ref().ok_or_else(|| {
                format!("strategy={:?} requires [retrieval.vector]", cfg.retrieval.strategy)
            })?;
            let api_key = std::env::var(&v.api_key_env)
                .map_err(|_| format!("[retrieval.vector]: env {:?} is not set", v.api_key_env))?;
            let openai = embedder::OpenAiEmbedder::new(
                v.base_url.clone(),
                v.model.clone(),
                api_key,
                v.dim,
                v.timeout_ms.map(std::time::Duration::from_millis),
            );
            backends.embedder = Some(Arc::new(retrieval::CachingEmbedder::new(Arc::new(openai))));
        }
        "subagent" => {
            let s = cfg
                .retrieval
                .subagent
                .as_ref()
                .ok_or("strategy=\"subagent\" requires [retrieval.subagent]")?;
            let api_key = std::env::var(&s.api_key_env)
                .map_err(|_| format!("[retrieval.subagent]: env {:?} is not set", s.api_key_env))?;
            let openai = chat::OpenAiChat::new(
                s.base_url.clone(),
                s.model.clone(),
                api_key,
                s.timeout_ms.map(std::time::Duration::from_millis),
            );
            backends.chat = Some(Arc::new(openai));
            backends.subagent_candidates = s.candidates;
        }
        _ => {}
    }
    Ok(backends)
}
```

- [ ] **Step 3: `prepare_state` 用 `with_backends`**

把 `prepare_state` 里构造 `state` 的那段：
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
替换为：
```rust
    let backends = build_backends(cfg)?;
    let state = Arc::new(
        gateway::GatewayState::with_backends(&cfg.retrieval.strategy, backends)
            .map_err(|e| e.to_string())?,
    );
```

- [ ] **Step 4: 更新 mcpgw 测试（重命名 + 新增 subagent）**

把 `tests` 模块里 4 个 `build_embedder_*` 测试改写为 `build_backends_*`：
```rust
    #[test]
    fn build_backends_empty_for_bm25() {
        let cfg = config::Config::default_from_empty();
        let b = build_backends(&cfg).unwrap();
        assert!(b.embedder.is_none() && b.chat.is_none());
    }

    #[test]
    fn build_backends_fails_fast_on_missing_vector_key() {
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_NO_KEY\"\n",
        )
        .unwrap();
        assert!(build_backends(&cfg).is_err());
    }

    #[test]
    fn build_backends_embedder_for_vector_with_key() {
        std::env::set_var("MCPGW_M2_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"vector\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2_KEY\"\n",
        )
        .unwrap();
        assert!(build_backends(&cfg).unwrap().embedder.is_some());
    }

    #[test]
    fn build_backends_embedder_for_hybrid_with_key() {
        std::env::set_var("MCPGW_M2B_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"hybrid\"\n[retrieval.vector]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2B_KEY\"\n",
        )
        .unwrap();
        assert!(build_backends(&cfg).unwrap().embedder.is_some());
    }

    #[test]
    fn build_backends_chat_for_subagent_with_key() {
        std::env::set_var("MCPGW_M2T5_KEY", "sk-x");
        let cfg = config::Config::from_toml_str(
            "[retrieval]\nstrategy=\"subagent\"\n[retrieval.subagent]\nmodel=\"m\"\napi_key_env=\"MCPGW_M2T5_KEY\"\ncandidates=15\n",
        )
        .unwrap();
        let b = build_backends(&cfg).unwrap();
        assert!(b.chat.is_some() && b.embedder.is_none());
        assert_eq!(b.subagent_candidates, Some(15));
    }
```
（若有别的测试引用 `build_embedder`，一并改名为 `build_backends`。）

- [ ] **Step 5: gateway —— subagent 快照测试**

在 `crates/gateway/src/lib.rs` 的 `tests` 模块，紧跟 hybrid 快照测试之后新增：
```rust
    #[tokio::test]
    async fn with_backends_rebuild_builds_subagent_snapshot_no_upstreams() {
        let backends = retrieval::Backends {
            chat: Some(std::sync::Arc::new(retrieval::MockChatModel::new("[]"))),
            ..Default::default()
        };
        let state =
            super::GatewayState::with_backends("subagent", backends).expect("subagent state");
        state.rebuild_snapshot().await.expect("rebuild ok");
        assert!(metatools::search_tools(&state.snapshot(), "anything", 5)
            .await
            .is_empty());
    }
```

- [ ] **Step 6: 运行 + fmt + clippy**

```bash
cargo test -p mcpgw -p gateway --all-features
cargo fmt -p mcpgw -p gateway && cargo fmt -p mcpgw -p gateway -- --check
cargo clippy -p mcpgw -p gateway --all-targets --all-features
```
Expected: 全 PASS（含 `build_backends_*` 5 条、gateway subagent 快照）。

- [ ] **Step 7: 提交**

```bash
git add crates/mcpgw/Cargo.toml crates/mcpgw/src/main.rs crates/gateway/src/lib.rs
git commit -m "feat(mcpgw,gateway): build_backends wires chat for subagent (M2.T5 T7)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 8: 分层文档（L1–L4 + README + roadmap）

docs 必须忠实描述已落地代码——动手前先读对应源码与现有 doc 文件，沿用其风格。

**Files:**
- Create: `docs/L4-api/retrieval-subagent.md`、`docs/L4-api/chat-openai.md`、`docs/L2-components/chat.md`
- Modify: `docs/L4-api/retrieval-lib.md`、`docs/L4-api/config-lib.md`、`docs/L4-api/mcpgw-main.md`、`docs/L3-details/retrieval.md`、`docs/L2-components/retrieval.md`、`docs/L2-components/config.md`、`docs/L1-overview.md`、`docs/README.md`、`docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`

- [ ] **Step 1: 新建 L4 `docs/L4-api/retrieval-subagent.md`**

内容覆盖（用与 `retrieval-hybrid.md` 同样的版式）：源文件 `crates/retrieval/src/subagent.rs`；`pub struct SubagentStrategy`（私有 `bm25`/`chat`/`candidates`）；`SubagentStrategy::new(chat: Arc<dyn ChatModel>, candidates: usize)`（`candidates.max(1)`）；`pub const DEFAULT_CANDIDATES = 20`；`index` 建内部 BM25；`search` 流程：BM25 预筛 `candidates` → 空则返回空（chat 不调用）→ 构造 system/user prompt → `chat.complete` → `parse_selection`（截取首个 `[...]`、JSON 解析、**只保留 shortlist 内的名字**、去重保序）→ 失败/空则降级回 BM25 shortlist（截 top_k）→ 否则映射成 `ScoredTool`（合成递减分，仅排序用，不可跨策略比较）。注明私有 `build_user_prompt`/`parse_selection` 不属公开 API。

- [ ] **Step 2: 新建 L4 `docs/L4-api/chat-openai.md`**

版式同 `embedder-openai.md`：源文件 `crates/chat/src/lib.rs`；`pub struct OpenAiChat`；`OpenAiChat::new(base_url, model, api_key, timeout: Option<Duration>)`（`base_url` 去尾 `/`）；`impl retrieval::ChatModel`：POST `{base_url}/chat/completions`，body `{model, temperature:0, messages:[system,user]}`，Bearer；取 `choices[0].message.content`；非 2xx → `ChatError::Provider`（附 ≤500 字 body 片段）；无 choices/空内容 → `ChatError::Empty`；解码失败 → `Provider`。注明这是工作区第二个、也是仅有的另一个带 reqwest 的 crate。

- [ ] **Step 3: 更新 L4 `retrieval-lib.md`**

补 `ChatModel`/`ChatError`（见 chat.rs）；`StrategyError` 加 `ChatModelRequired`；`Backends { embedder, chat, subagent_candidates }`；`build_strategy` 新签名 `(name: &str, backends: &Backends)` 与四臂（bm25 / vector·hybrid 需 embedder / subagent 需 chat→ChatModelRequired / 其它 NotImplemented）；页脚补 `> 逐文件 subagent API 见 [retrieval/subagent.rs](./retrieval-subagent.md)`。`SubagentStrategy` 已在 subagent L4，列一行引用即可。

- [ ] **Step 4: 更新 L4 `config-lib.md` 与 `mcpgw-main.md`**

- `config-lib.md`：新增 `SubagentConfig`（base_url 默认 OpenAI/model/api_key_env/timeout_ms?/candidates?，`deny_unknown_fields`）；`RetrievalConfig` 加 `subagent: Option<SubagentConfig>`；`validate` 描述与 `Invalid` 项补「`strategy="subagent"` 缺 `[retrieval.subagent]` 或字段空白、`candidates==0`」；`KNOWN` 现为 4 项。
- `mcpgw-main.md`：`fn build_embedder` 行改为 `fn build_backends`：「按 `retrieval.strategy` 建后端：vector/hybrid → embedder（OpenAiEmbedder+CachingEmbedder）；subagent → chat（OpenAiChat）；bm25 → 空 `Backends`；启动期 fail-fast 读 key」；`prepare_state` 行改为「`build_backends` → `GatewayState::with_backends`」。

- [ ] **Step 5: 更新 L3 `docs/L3-details/retrieval.md`**

标题可改为 `（BM25 / 向量 / 混合 / subagent）`；末尾追加「subagent 策略」小节：retrieve-then-rerank 数据流（BM25 预筛 candidates → prompt → chat → 解析白名单过滤/去重 → 合成名次分 → 截 top_k）；**空 shortlist 局限**（固定 BM25 预筛：纯语义无字面命中 → 空，chat 不调用）；幻觉过滤；**降级自愈**（complete/解析失败 → 回 BM25 shortlist）；prompt/解析为何放 retrieval（纯逻辑、MockChatModel 可测）。

- [ ] **Step 6: 更新 L2**

- `docs/L2-components/retrieval.md`：职责补 `SubagentStrategy`；新增 `### 抽象 ChatModel`（system+user→text，HTTP 实现在 `chat` crate）、`### 类型 Backends`（embedder/chat/subagent_candidates，注入 build_strategy）、`### 类型 SubagentStrategy`（BM25 预筛 + chat 重排 + 降级，详见 L3/L4）；`StrategyError` 列 `ChatModelRequired`；`build_strategy` 项更新为四臂 + Backends；向下导航 L4 加 subagent。依赖处补：现也依赖 `serde_json`（解析 LLM JSON）。
- 新建 `docs/L2-components/chat.md`（仿 `embedder.md`）：`OpenAiChat` 实现 `retrieval::ChatModel`，HTTP/reqwest 只在此 crate；被 `mcpgw` 在 `strategy="subagent"` 时构造注入。
- `docs/L2-components/config.md`：`subagent` 行 + `SubagentConfig` 表；`strategy ∈ {vector,hybrid}` 必填 vector、`strategy="subagent"` 必填 subagent；`Invalid` 项补 subagent。
- `docs/L3-details/mcpgw-cli.md`：离线 `search` 对 `strategy="subagent"` 也无后端 → `ChatModelRequired`（与 vector/hybrid 的 EmbedderRequired 对称）——按需补一句。

- [ ] **Step 7: 更新 L1 `docs/L1-overview.md`**

新增 `chat` crate（工作区第二个带 HTTP 依赖，与 `embedder` 对称）；新增 `subagent` 检索策略（BM25 预筛→小模型重排，opt-in）；`build_strategy` 改为 `Backends` 注入；**默认仍 `bm25`**。若有「crate 个数 / 架构图 / 装配图」涉及 embedder 的描述，补上 chat 的对称项与 `build_backends`。新增一段 M2.T5 里程碑小结（仿 M2-A/M2-B 的「已完成」条目）。保持「默认 bm25」表述不变。

- [ ] **Step 8: 更新 `docs/README.md` 与 roadmap**

- `README.md`：L2 清单加 `chat`；L4 清单加 `chat-openai.md`、`retrieval-subagent.md`；里程碑覆盖说明加 **M2.T5（subagent 重排）**。
- roadmap `2026-06-08-mcpgw-program-roadmap.md`：`M2.T5 — SubagentStrategy（可选）` 标 `✅ 已完成（默认仍 bm25，opt-in）`。

- [ ] **Step 9: 校对 + 提交**

- 逐项核对新/改 doc 与真实代码一致（subagent.rs、chat/src/lib.rs、config、main.rs、lib.rs）；确认新增内链指向真实文件（`ls docs/L4-api/retrieval-subagent.md docs/L4-api/chat-openai.md docs/L2-components/chat.md`）。
- 全指南无「subagent 未实现 / 待 M2.T5」遗留。

```bash
git add docs/
git commit -m "docs: L1-L4 + README + roadmap for M2.T5 subagent retrieval (M2.T5 T8)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 9: 全量验证 + 合回 master

- [ ] **Step 1: 全量验证**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净；clippy 无告警；全测试 PASS（含 retrieval `subagent`/`chat`/`vector`/`hybrid` testkit 目标、`chat` crate 测试；`#[ignore]` 真实冒烟被跳过）。

- [ ] **Step 2: 收尾**

1. 派发最终整体 code review（spec 覆盖、Backends 重构无回归、subagent 解析/降级正确、HTTP 客户端、密钥仅 env、文档同步）。
2. 处理 blocking 项（如有）。
3. 用 superpowers:finishing-a-development-branch 把 `feat/m2t5-subagent-retrieval` 合回 master（`--no-ff`，本地），删分支。

## 实现期需现场确认/可能回退的点（spec §9）
- `build_strategy` 签名从 `(name, Option<&Arc<dyn Embedder>>)` 改为 `(name, &Backends)`：全调用点（gateway/mcpgw/retrieval 测试）编译实证。
- `serde_json` 提为 `retrieval` 正式依赖（解析 LLM JSON）；确认未引入 HTTP 依赖。
- LLM 响应解析鲁棒性：v1「截首个 `[...]` + 白名单过滤」，失败即降级；真实冒烟若显示需更宽松解析再迭代（「失败必降级」不变）。
- `rerank_follows_model_order` 依赖 BM25 对 `"create github issue"` 的实际 shortlist（两个 github 工具）；若不符先核对、勿弱化断言。
- 合成名次分仅本策略内排序与 `top_k` 截断用；downstream 只用顺序（与 hybrid 一致）。
