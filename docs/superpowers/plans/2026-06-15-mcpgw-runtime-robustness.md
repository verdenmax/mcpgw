# mcpgw 运行态健壮性整改 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复审计的 5 项运行态 Minor 问题：缓存 hash 碰撞键、ingest 任务 panic 崩溃隔离、上游 `isError` 记为 ok、HTTP 缺优雅关闭、`Arc::try_unwrap` 收尾跳过 cancel。

**Architecture:** 局部硬化，不改公开架构：retrieval 缓存键改 `String`；gateway rebuild 把 `JoinError` 降级为 skip；downstream 按 `isError` 分类；mcpgw `run_serve` 把 HTTP 改为带 `with_graceful_shutdown` 的后台任务（oneshot 驱动）+ 有界排空；upstream 加非消费式 `cancel(&self)`。不新增 crate 依赖。

**Tech Stack:** Rust，std `tokio::sync::oneshot`，rmcp `RunningService::cancellation_token`，既有 testkit/CaptureSink/serve 测试框架。

> 设计依据：`docs/superpowers/specs/2026-06-15-mcpgw-runtime-robustness-design.md`。其余 5 项 Minor 暂缓。

---

## File Structure

| 文件 | 动作 | 整改项 |
|------|------|--------|
| `crates/retrieval/src/caching.rs` | 重写（键 `u64`→`String`） | M1 |
| `crates/upstream/src/testkit.rs` | 加 `fail` 工具 | M3（测试支撑）|
| `crates/downstream/src/lib.rs` | `Ok(result)` 臂按 `is_error` 分类 + 测试 | M3 |
| `crates/downstream/tests/server.rs` | 加 isError 观测测试 | M3 |
| `crates/gateway/src/lib.rs` | `JoinError`→skip+warn | M2 |
| `crates/upstream/src/connection.rs` | `cancel(&self)` | M5 |
| `crates/mcpgw/src/main.rs` | 收尾 cancel（M5）+ HTTP 优雅关闭重构（M4）| M5/M4 |
| `docs/L3-details/*`、`docs/L4-api/*`、`docs/L1-overview.md` | 分层文档 | 全部 |

**前置：建分支**

```bash
cd /home/verden/course/mcpgw
git checkout master
git checkout -b fix/runtime-robustness
```

---

### Task 1: M1 缓存键改为文本 `String`

**Files:** Modify (rewrite) `crates/retrieval/src/caching.rs`.

- [ ] **Step 1: 重写 `caching.rs`（键 `String`）+ 测试**

把 `crates/retrieval/src/caching.rs` 整体替换为：

```rust
//! `CachingEmbedder`: an `Embedder` decorator that memoizes vectors by text content.
//!
//! Bounded by a two-generation scheme (`current` + `previous`, each capped at
//! `CACHE_GEN_CAP`) so memory cannot grow without bound when arbitrary query texts are
//! embedded. Frequently-seen texts (e.g. tool descriptions re-embedded each rebuild) stay
//! warm via promote-on-hit. Only cache-miss texts are forwarded to `inner`. Keyed on the text
//! itself, so two distinct texts can never collide onto one cache slot.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::embedder::{EmbedError, Embedder};

/// Per-generation entry cap. Total resident entries are bounded by ~`2 * CACHE_GEN_CAP`.
const CACHE_GEN_CAP: usize = 2048;

/// Two-generation bounded cache keyed on the text. Lookups check `current`, then `previous`
/// (promoting a `previous` hit back into `current`). When `current` reaches `CACHE_GEN_CAP`, it
/// rotates into `previous` (dropping the old `previous`) and a fresh `current` starts.
struct GenCache {
    current: HashMap<String, Arc<[f32]>>,
    previous: HashMap<String, Arc<[f32]>>,
}

impl GenCache {
    fn new() -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
        }
    }

    /// Look up `key`, promoting a `previous`-generation hit into `current`.
    fn get(&mut self, key: &str) -> Option<Arc<[f32]>> {
        if let Some(v) = self.current.get(key) {
            return Some(v.clone());
        }
        if let Some(v) = self.previous.remove(key) {
            self.insert(key.to_string(), v.clone());
            return Some(v);
        }
        None
    }

    /// Insert `key`, rotating generations first if `current` is full.
    fn insert(&mut self, key: String, value: Arc<[f32]>) {
        if self.current.len() >= CACHE_GEN_CAP && !self.current.contains_key(&key) {
            self.previous = std::mem::take(&mut self.current);
        }
        self.current.insert(key, value);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }
}

/// Memoizes embeddings by text content, bounded by a two-generation cache.
pub struct CachingEmbedder {
    inner: Arc<dyn Embedder>,
    cache: Mutex<GenCache>,
}

impl CachingEmbedder {
    pub fn new(inner: Arc<dyn Embedder>) -> Self {
        Self {
            inner,
            cache: Mutex::new(GenCache::new()),
        }
    }
}

#[async_trait]
impl Embedder for CachingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        // First pass: pull cached vectors (promoting previous-gen hits) into a local map and
        // collect the unique miss texts. The lock is held only for synchronous map ops.
        let mut resolved: HashMap<String, Arc<[f32]>> = HashMap::new();
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_seen: HashSet<String> = HashSet::new();
        {
            let mut cache = self.cache.lock().unwrap();
            for t in texts {
                if resolved.contains_key(t) {
                    continue;
                }
                if let Some(v) = cache.get(t) {
                    resolved.insert(t.clone(), v);
                } else if miss_seen.insert(t.clone()) {
                    miss_texts.push(t.clone());
                }
            }
        }

        // Embed only the misses (skip the call entirely if everything was cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            let mut cache = self.cache.lock().unwrap();
            for (t, v) in miss_texts.iter().zip(embedded) {
                let arc: Arc<[f32]> = Arc::from(v.into_boxed_slice());
                cache.insert(t.clone(), arc.clone());
                resolved.insert(t.clone(), arc);
            }
        }

        // Reassemble in original input order from the local `resolved` map (NOT the bounded
        // cache, which may have evicted entries within an oversized batch). Every text is either a
        // first-pass hit or was just embedded+inserted, so the lookup cannot miss — unless the
        // inner embedder returned fewer vectors than inputs (a contract violation the only
        // production `Embedder` rejects as `Err`).
        Ok(texts
            .iter()
            .map(|t| resolved.get(t).expect("text resolved above").to_vec())
            .collect())
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Local FNV-1a, used only to derive deterministic test vectors (equal text -> equal vector).
    fn text_hash(text: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in text.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Embedder that counts how many texts it was asked to embed and returns a deterministic
    /// vector derived from each text (so equal texts -> equal vectors, distinct texts differ).
    struct CountingEmbedder {
        calls: AtomicUsize,
        dim: usize,
    }
    impl CountingEmbedder {
        fn new(dim: usize) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                dim,
            }
        }
    }
    #[async_trait]
    impl Embedder for CountingEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.calls.fetch_add(texts.len(), Ordering::Relaxed);
            Ok(texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0f32; self.dim];
                    v[0] = text_hash(t) as f32;
                    v
                })
                .collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn caches_hits_and_only_embeds_misses() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        let a = c.embed(&["x".into()]).await.unwrap();
        let b = c.embed(&["x".into()]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(
            inner.calls.load(Ordering::Relaxed),
            1,
            "second embed of the same text must hit the cache (no new inner call)"
        );
    }

    #[tokio::test]
    async fn distinct_texts_get_distinct_vectors() {
        // With a text key, distinct texts can never share a slot. Embed two different texts and
        // confirm each keeps its own vector across a re-embed (no cross-contamination).
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner);
        let first = c.embed(&["alpha".into(), "beta".into()]).await.unwrap();
        let again = c.embed(&["alpha".into(), "beta".into()]).await.unwrap();
        assert_eq!(first, again);
        assert_ne!(first[0], first[1], "different texts must map to different vectors");
    }

    #[tokio::test]
    async fn memory_is_bounded_under_many_distinct_texts() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner);
        for i in 0..(CACHE_GEN_CAP * 3) {
            c.embed(&[format!("q{i}")]).await.unwrap();
        }
        let cache = c.cache.lock().unwrap();
        assert!(
            cache.len() <= 2 * CACHE_GEN_CAP,
            "cache must stay bounded at ~2*CAP, got {}",
            cache.len()
        );
    }

    #[tokio::test]
    async fn promote_on_hit_prevents_re_embedding_a_hot_key() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        c.embed(&["hot".into()]).await.unwrap();
        for i in 0..(CACHE_GEN_CAP * 3) {
            c.embed(&[format!("k{i}")]).await.unwrap();
            if i % (CACHE_GEN_CAP / 2) == 0 {
                c.embed(&["hot".into()]).await.unwrap();
            }
        }
        assert_eq!(
            inner.calls.load(Ordering::Relaxed),
            CACHE_GEN_CAP * 3 + 1,
            "promotion must prevent any re-embed of the periodically-touched hot key"
        );
    }

    #[tokio::test]
    async fn single_oversized_batch_stays_bounded_and_returns_all_vectors() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        let batch: Vec<String> = (0..(CACHE_GEN_CAP * 2 + 7))
            .map(|i| format!("t{i}"))
            .collect();
        let out = c.embed(&batch).await.unwrap();

        assert_eq!(out.len(), batch.len(), "one output vector per input");
        for (t, v) in batch.iter().zip(&out) {
            let mut expected = vec![0.0f32; 2];
            expected[0] = text_hash(t) as f32;
            assert_eq!(v, &expected, "vector for {t:?} must be correct despite mid-batch eviction");
        }
        assert!(
            c.cache.lock().unwrap().len() <= 2 * CACHE_GEN_CAP,
            "persistent cache must stay bounded even for an oversized batch"
        );
    }
}
```

- [ ] **Step 2: 跑测试看绿**

Run: `cargo test -p retrieval caching && cargo test -p retrieval --all-features`
Expected: 5 个 caching 单测 PASS；既有集成 caching（4）+ vector/hybrid/subagent 全绿（它们用 `CachingEmbedder`，公开行为未变）。

- [ ] **Step 3: fmt + clippy**

Run: `cargo fmt -p retrieval && cargo fmt --check -p retrieval && cargo clippy -p retrieval --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 4: Commit**

```bash
git add crates/retrieval/src/caching.rs
git commit -m "fix(retrieval): key the embedding cache on text, not a u64 hash (audit M1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 2: M3 上游 `isError` 记为 `outcome=Error`

**Files:**
- Modify: `crates/upstream/src/testkit.rs`（加 `fail` 工具）
- Modify: `crates/downstream/src/lib.rs`（`Ok(result)` 臂按 `is_error` 分类）
- Modify: `crates/downstream/tests/server.rs`（观测测试）

- [ ] **Step 1: testkit 加 `fail` 工具**

在 `crates/upstream/src/testkit.rs` 的 `#[tool_router] impl MockUpstream` 内（`slow` 之后）加：

```rust
    #[tool(description = "Always returns a tool-level error (is_error=true)")]
    fn fail(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::error(vec![Content::text(
            "tool failed on purpose",
        )]))
    }
```

- [ ] **Step 2: 写失败的观测测试**

先读 `crates/downstream/tests/server.rs` 的 `meta_tool_calls_are_observed_with_metadata`（用 `CaptureSink` + `attach_mock`）。在该文件追加（仿其 setup）：

```rust
#[tokio::test]
async fn upstream_tool_error_is_recorded_as_error_outcome() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    let r = client
        .call_tool(
            rmcp::model::CallToolRequestParams::new("call_tool")
                .with_arguments(args(serde_json::json!({"name": "mock__fail"}))),
        )
        .await
        .unwrap();
    // The tool-level error is forwarded to the client unchanged.
    assert_eq!(r.is_error, Some(true));
    client.cancel().await.unwrap();

    let recs = cap.records();
    let rec = recs.last().expect("a record for the call");
    assert_eq!(rec.meta_tool, MetaTool::CallTool);
    assert_eq!(rec.target_tool.as_deref(), Some("mock__fail"));
    assert_eq!(rec.outcome, CallOutcome::Error);
    assert_eq!(rec.error_kind, Some("upstream_tool_error"));
}
```

> 注：`args(...)` 辅助函数已存在于该文件；`observe` 与 `rmcp` 已是 dev/常规依赖。若 `Arc`/`use` 缺失按文件现有 import 补齐。

- [ ] **Step 3: 跑测试看失败**

Run: `cargo test -p downstream --test server upstream_tool_error_is_recorded`
Expected: 失败——当前 `is_error=true` 的结果被记为 `outcome=Ok`/`error_kind=None`。

- [ ] **Step 4: 实现 M3 分类**

在 `crates/downstream/src/lib.rs` 的 `call_tool` 里，把 `call_tool` 元工具成功臂：

```rust
                        Ok(result) => (
                            Ok(result),
                            MetaTool::CallTool,
                            Some(name.to_string()),
                            CallOutcome::Ok,
                            None,
                        ),
```

替换为：

```rust
                        Ok(result) => {
                            // A successful round-trip whose result carries is_error=true is a
                            // tool-level failure: forward it unchanged, but record it as an error
                            // so the audit/metrics don't undercount tool failures.
                            let (outcome, kind) = if result.is_error == Some(true) {
                                (CallOutcome::Error, Some("upstream_tool_error"))
                            } else {
                                (CallOutcome::Ok, None)
                            };
                            (
                                Ok(result),
                                MetaTool::CallTool,
                                Some(name.to_string()),
                                outcome,
                                kind,
                            )
                        }
```

- [ ] **Step 5: 跑测试看绿 + 回归**

Run: `cargo test -p downstream --all-features && cargo test -p upstream --all-features`
Expected: 新测试 PASS；既有 downstream/upstream 测试（含 `meta_tool_calls_are_observed_with_metadata`、call_tool 路由）全绿。

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt -p downstream -p upstream && cargo fmt --check -p downstream -p upstream && cargo clippy -p downstream -p upstream --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 7: Commit**

```bash
git add crates/upstream/src/testkit.rs crates/downstream/src/lib.rs crates/downstream/tests/server.rs
git commit -m "fix(downstream): record upstream is_error results as Error/upstream_tool_error (audit M3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 3: M2 `ingest` 任务 panic 降级为 skip

**Files:** Modify `crates/gateway/src/lib.rs`（`rebuild_snapshot` 的 `join_next` 循环）。

- [ ] **Step 1: 实现 M2**

在 `crates/gateway/src/lib.rs` 的 `rebuild_snapshot` 里，把：

```rust
        while let Some(joined) = set.join_next().await {
            let (name, outcome, local) = joined.expect("ingest task panicked");
```

替换为：

```rust
        while let Some(joined) = set.join_next().await {
            // A panicked/cancelled ingest task must NOT crash the (initial) build or kill the
            // rebuild worker — degrade it to a skipped upstream so crash isolation holds. The
            // panicked task's upstream name is unrecoverable, so record a generic entry.
            let (name, outcome, local) = match joined {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "ingest task panicked/cancelled; skipping");
                    summary
                        .skipped
                        .push(("<ingest task>".to_string(), format!("task failed: {e}")));
                    continue;
                }
            };
```

- [ ] **Step 2: 跑回归测试**

Run: `cargo test -p gateway --all-features`
Expected: 既有 gateway/rebuild 测试全绿（正常 rebuild 路径不受影响；`Ok` 分支等价于原 `expect`）。

> 测试局限（如实记录）：从公开 API 注入「客户端侧 ingest 任务 panic」很困难（需 rmcp 在回包上 panic），故本项为防御性改动，靠「正常路径不回归 + 分支逻辑显然」验收，不新增专用 panic-注入测试。

- [ ] **Step 3: fmt + clippy**

Run: `cargo fmt -p gateway && cargo fmt --check -p gateway && cargo clippy -p gateway --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 4: Commit**

```bash
git add crates/gateway/src/lib.rs
git commit -m "fix(gateway): degrade a panicked ingest task to a skipped upstream, not a crash (audit M2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: M5 上游收尾改用非消费式 `cancel(&self)`

**Files:**
- Modify: `crates/upstream/src/connection.rs`（加 `cancel(&self)` + 测试）
- Modify: `crates/mcpgw/src/main.rs`（收尾循环）

- [ ] **Step 1: 写失败/支撑测试（upstream）**

先读 `crates/upstream/tests/integration.rs` 看它如何用 `MockUpstream` 建 `UpstreamHandle`（in-memory duplex）。追加一个测试，验证 `cancel(&self)` 可在**共享 `Arc`** 上调用且使服务不可用：

```rust
#[tokio::test]
async fn cancel_works_on_a_shared_handle() {
    // Build a handle to a MockUpstream over an in-memory duplex, share it via Arc + clone, and
    // cancel through the clone (the teardown path when try_unwrap can't take ownership).
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = upstream::testkit::MockUpstream::new().serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = std::sync::Arc::new(
        upstream::connection::UpstreamHandle::connect("mock", client_io)
            .await
            .unwrap(),
    );
    let shared = handle.clone();

    shared.cancel(); // must not panic, works via &self on a shared Arc

    // After cancellation the service is gone, so a forwarded call fails (no hang).
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        handle.call_tool("echo", None),
    )
    .await
    .expect("call must not hang after cancel");
    assert!(r.is_err(), "calls must fail once the service is cancelled");
}
```

> 注：`integration.rs` 已 `required-features=["testkit"]`；按文件现有 `use` 风格补 `ServiceExt` 等导入（`MockUpstream::new().serve` 需要 `rmcp::ServiceExt`）。若现有 helper 已封装连接，复用之。

- [ ] **Step 2: 跑测试看失败**

Run: `cargo test -p upstream --all-features cancel_works_on_a_shared_handle`
Expected: 编译失败——`UpstreamHandle::cancel` 尚未定义。

- [ ] **Step 3: 实现 `cancel(&self)`**

在 `crates/upstream/src/connection.rs` 的 `impl UpstreamHandle` 中，紧邻 `shutdown` 加：

```rust
    /// Cancel the underlying rmcp service via its cancellation token, WITHOUT consuming the
    /// handle. Unlike `shutdown(self)`, this works on a shared `&self` (e.g. an
    /// `Arc<UpstreamHandle>` still held by the rebuild worker or an in-flight call), so teardown
    /// never silently skips a cancel. The child is ultimately reaped by the service's DropGuard
    /// when the last clone drops.
    pub fn cancel(&self) {
        self.client.cancellation_token().cancel();
    }
```

- [ ] **Step 4: 跑测试看绿**

Run: `cargo test -p upstream --all-features`
Expected: `cancel_works_on_a_shared_handle` PASS；既有 upstream 测试全绿。

- [ ] **Step 5: 改 `mcpgw` 收尾循环（不再 `try_unwrap` 即跳过）**

在 `crates/mcpgw/src/main.rs` 的收尾循环，把：

```rust
    // Best-effort graceful shutdown of upstream children (runs on clean exit AND error).
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            if let Ok(h) = Arc::try_unwrap(handle) {
                h.shutdown().await;
            }
        }
    }
```

替换为：

```rust
    // Graceful shutdown of upstream children (runs on clean exit AND error). If we own the only
    // reference, await a full graceful cancel; otherwise (rebuild worker / in-flight call still
    // holds a clone) cancel via the service token so the upstream is never silently left running.
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            match Arc::try_unwrap(handle) {
                Ok(h) => h.shutdown().await,
                Err(shared) => shared.cancel(),
            }
        }
    }
```

- [ ] **Step 6: 跑 mcpgw 测试 + fmt + clippy**

Run: `cargo test -p mcpgw -p upstream --all-features && cargo fmt -p mcpgw -p upstream && cargo fmt --check -p mcpgw -p upstream && cargo clippy -p mcpgw -p upstream --all-targets --all-features -- -D warnings`
Expected: 全绿、干净、无告警。

- [ ] **Step 7: Commit**

```bash
git add crates/upstream/src/connection.rs crates/mcpgw/src/main.rs
git commit -m "fix(upstream,mcpgw): cancel shared upstream handles on teardown via the rmcp token (audit M5)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: M4 HTTP 优雅关闭（`with_graceful_shutdown`）

**Files:**
- Modify: `crates/mcpgw/src/main.rs`（`run_serve`：HTTP 后台任务 + oneshot 优雅关闭 + 有界排空 + 收尾顺序；新增常量）
- Modify: `crates/downstream/tests/http_server.rs`（优雅关闭机制测试）

- [ ] **Step 1: 写优雅关闭机制测试（downstream http）**

在 `crates/downstream/tests/http_server.rs` 追加（验证 `with_graceful_shutdown(oneshot)` 能及时停服）：

```rust
#[tokio::test]
async fn http_graceful_shutdown_stops_the_server_promptly() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let sinks: std::sync::Arc<[std::sync::Arc<dyn observe::CallSink>]> = Vec::new().into();
    let router = downstream::http::build_router(state, 8, "/mcp", vec![], sinks);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
    });

    // Fire graceful shutdown; the serve task must finish promptly (well under the timeout).
    tx.send(()).unwrap();
    let res = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    assert!(res.is_ok(), "graceful shutdown must stop the server promptly");
    assert!(res.unwrap().unwrap().is_ok(), "serve returned an error");
}
```

- [ ] **Step 2: 跑测试看绿（机制本身已被 axum 支持）**

Run: `cargo test -p downstream --test http_server http_graceful_shutdown_stops_the_server_promptly`
Expected: PASS（这是对 axum `with_graceful_shutdown` 机制的护栏测试；M4 的价值在于把它接进 `run_serve`，下面实现）。

- [ ] **Step 3: 加常量 `HTTP_SHUTDOWN_TIMEOUT`**

在 `crates/mcpgw/src/main.rs` 顶部，`AUDIT_DRAIN_TIMEOUT` 旁加：

```rust
/// Upper bound on how long shutdown waits for the HTTP server to drain in-flight requests.
const HTTP_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
```

- [ ] **Step 4: 重构 `run_serve` 的 HTTP/关闭区块**

在 `crates/mcpgw/src/main.rs` 中，把现有从 `let stdio_enabled = cfg.server.stdio;` 到审计 drain 结束（即 `drop(sinks); if let Some(writer) = audit_writer { ... }` 那段）整体替换为：

```rust
    let stdio_enabled = cfg.server.stdio;
    let state_for_stdio = state.clone();
    let top_k = cfg.retrieval.top_k;

    // Run HTTP as a background task with graceful shutdown driven by a oneshot, so on shutdown its
    // keep-alive sessions close and release their GatewayServer/JsonlSink clones promptly (instead
    // of being orphaned and forcing the audit drain to wait out its timeout).
    let (http_shutdown_tx, http_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut http_task = http_bound.map(|(listener, router)| {
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = http_shutdown_rx.await;
                })
                .await
                .map_err(|e| e.to_string())
        })
    });

    // Wait for the first shutdown trigger: stdio client disconnect, ctrl_c, or the HTTP server
    // terminating on its own (a bind/serve error, or — in HTTP-only mode — the only transport).
    let mut http_self_terminated = false;
    let outcome: Result<(), String> = tokio::select! {
        res = async {
            let server = downstream::GatewayServer::new(state_for_stdio, top_k, sinks.clone());
            let service = server.serve(stdio()).await.map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())
        }, if stdio_enabled => {
            if res.is_ok() {
                tracing::info!("stdio client disconnected; shutting down");
            }
            res.map(|_| ())
        }
        res = async {
            match http_task.as_mut() {
                Some(t) => t.await.map_err(|e| e.to_string()).and_then(|r| r),
                None => std::future::pending().await,
            }
        }, if http_enabled => {
            http_self_terminated = true;
            res
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c; shutting down");
            Ok(())
        }
    };

    // Signal graceful shutdown and await the HTTP drain (bounded), UNLESS the HTTP task already
    // ended (then its JoinHandle is consumed and must not be awaited again). Draining here closes
    // keep-alive sessions and releases their sink clones before the audit drain below.
    let _ = http_shutdown_tx.send(());
    if !http_self_terminated {
        if let Some(task) = http_task {
            if tokio::time::timeout(HTTP_SHUTDOWN_TIMEOUT, task).await.is_err() {
                tracing::warn!("http server graceful shutdown timed out");
            }
        }
    }

    // Drain the audit writer (if any). With the HTTP sessions now closed and the stdio server
    // dropped, `drop(sinks)` releases the last JsonlSink clone, disconnecting the channel so the
    // writer FIFO-drains, flushes, fsyncs, and exits — promptly, not at the timeout.
    drop(sinks);
    if let Some(writer) = audit_writer {
        if tokio::time::timeout(
            AUDIT_DRAIN_TIMEOUT,
            tokio::task::spawn_blocking(move || writer.join()),
        )
        .await
        .is_err()
        {
            tracing::warn!("audit writer drain timed out; some records may be unflushed");
        }
    }
```

> 说明：`http_enabled` 守卫下，`http_task` 必为 `Some`（`http_bound` 仅在 http_enabled 时 `Some`），故 `None => pending()` 臂在该分支激活时不可达，仅为类型完整。`http_self_terminated` 防止对已完成的 `JoinHandle` 二次 `await`。

- [ ] **Step 5: 跑全量 mcpgw + downstream 测试**

Run: `cargo test -p mcpgw -p downstream --all-features`
Expected: 既有 stdio/http/audit e2e（`tests/audit.rs`、`http_server.rs`、`cli.rs`）全绿——三种传输模式（stdio-only / http-only / 双开）都不回归；新机制测试 PASS。

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt -p mcpgw -p downstream && cargo fmt --check -p mcpgw -p downstream && cargo clippy -p mcpgw -p downstream --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 7: Commit**

```bash
git add crates/mcpgw/src/main.rs crates/downstream/tests/http_server.rs
git commit -m "fix(mcpgw): graceful HTTP shutdown via oneshot + bounded drain; prompt audit drain (audit M4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 6: 分层文档同步

docs 必须忠实描述已落地代码。动手前读对应源码与现有 doc 风格。

**Files:**
- Modify: `docs/L3-details/downstream.md`、`docs/L4-api/downstream-lib.md`（M3 taxonomy）
- Modify: `docs/L3-details/retrieval.md`、`docs/L4-api/retrieval-embedder.md`（M1 缓存键）
- Modify: `docs/L3-details/gateway.md`（M2 ingest skip）
- Modify: `docs/L3-details/mcpgw-cli.md`、`docs/L4-api/mcpgw-main.md`（M4 优雅关闭 + M5 收尾）
- Modify: `docs/L4-api/upstream-connection.md`（M5 `cancel`）
- Modify: `docs/L1-overview.md`（测试计数块）

- [ ] **Step 1: M3 taxonomy**

`docs/L3-details/downstream.md` 与 `docs/L4-api/downstream-lib.md` 的 `error_kind` 取值表新增一行：`upstream_tool_error` —— `call_tool` 成功往返但上游结果 `is_error=true`（结果仍原样转发，仅观测记为 `outcome=Error`）。

- [ ] **Step 2: M1 缓存键**

`docs/L3-details/retrieval.md` 与 `docs/L4-api/retrieval-embedder.md`：把缓存键描述从「64 位内容哈希（碰撞概率可忽略）」改为「**文本 `String` 键，碰撞结构上不可能**」；删除哈希碰撞注脚；`GenCache` 字段类型更新为 `HashMap<String, Arc<[f32]>>`。

- [ ] **Step 3: M2 ingest skip**

`docs/L3-details/gateway.md`（rebuild 小节）：补一句——单个 ingest 任务 panic/取消现降级为 `skipped`（记 `"<ingest task>"` + `warn`），不再 `expect` 崩溃，维持崩溃隔离。

- [ ] **Step 4: M4/M5 关闭与收尾**

- `docs/L3-details/mcpgw-cli.md` 与 `docs/L4-api/mcpgw-main.md`：`serve` 的 HTTP 现为带 `with_graceful_shutdown`（oneshot 驱动）的后台任务；关闭顺序更新为「`select!` 触发 → `http_shutdown_tx.send` → 有界 `HTTP_SHUTDOWN_TIMEOUT` 排空 HTTP → `drop(sinks)` → 审计 drain → 上游收尾」；指出审计 drain 因 HTTP 会话先关而**及时完成**（不再总等满 5s）。上游收尾改为「独占 → `shutdown().await`，否则 → `cancel()`」。新增常量 `HTTP_SHUTDOWN_TIMEOUT`。
- `docs/L4-api/upstream-connection.md`：新增 `UpstreamHandle::cancel(&self)`（经 rmcp 取消令牌、非消费式）；`shutdown(self)` 仍在（独占、等待 quit）。

- [ ] **Step 5: L1 测试计数**

`docs/L1-overview.md` 测试计数块按实测更新：

```bash
cargo test --all-features 2>&1 | grep "test result:"
```

把新增测试计入（retrieval caching 由 4→5 个单测、downstream server +1、downstream http_server +1、upstream integration +1），重算总数与分项使其相加正确。

- [ ] **Step 6: 校对 + 提交**

- 逐项核对 doc 与真实代码（taxonomy、缓存键类型、gateway skip、关闭顺序/常量、`cancel` 签名）。
- 确认无产品文档仍称缓存键为「64 位哈希」、HTTP「无优雅关闭」、收尾「try_unwrap 即跳过」。

```bash
git add docs/
git commit -m "docs: sync layered docs for runtime robustness M1-M5

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 7: 全量验证 + 合回 master

- [ ] **Step 1: 全量验证**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
Expected: fmt 干净；clippy 无告警；全测试 PASS（含本次新增 retrieval/downstream/upstream 测试；`#[ignore]` 真实冒烟仍跳过）；lockfile 一致。记录总数复核 L1。

- [ ] **Step 2: 最终整体 code review**

派发最终 whole-feature review（用当前主会话模型），关注：5 项各自正确且无回归；缓存 String 键的去重/promote/有界仍正确、Mutex 不跨 `.await`；M3 仅改观测不改转发；M2 `Ok` 分支等价、`Err` 降级正确；M4 三种传输模式关闭都正确、`http_self_terminated` 防二次 await、收尾顺序使审计 drain 及时；M5 收尾对共享/独占两路都 cancel；文档同步。处理 blocking 项，小提交折叠 nits。

- [ ] **Step 3: 收尾合并**

用 superpowers:finishing-a-development-branch 把 `fix/runtime-robustness` 合回 master（`--no-ff`，本地），合并后在 master 复跑 `cargo test --all-features` 确认绿，再删分支。

## 实现期需现场确认/可能回退的点

- M4：`select!` 的 HTTP 分支用 `if http_enabled` 守卫并 `http_task.as_mut()` await；`http_self_terminated` 标志确保不对已完成 `JoinHandle` 二次 await。三模式（stdio-only / http-only / 双开）都要实测不回归——尤其 http-only（stdio=false）下关闭由 ctrl_c 或 HTTP 自结束驱动。
- M4 测试：对 `with_graceful_shutdown(oneshot)` 的护栏测试是机制级（在 `http_server.rs`，确定性、无需 SIGINT）；run_serve 的整体收尾顺序由既有 e2e 不回归 + 最终 review 把关。
- M5：`cancellation_token().cancel()` 为 fire-and-forget；`cancel_works_on_a_shared_handle` 测「取消后调用失败、不挂起」，如取消传播有时序，给 5s timeout 兜底。
- M2：无法注入「ingest 任务 panic」时以防御性改动 + 正常路径不回归验收。
- M1：测试 embedder 用本地 `text_hash` 派生确定性向量（不再依赖被删除的模块级 `hash_text`）。

