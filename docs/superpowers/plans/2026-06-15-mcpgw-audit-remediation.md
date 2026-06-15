# mcpgw 审计整改 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复一次全量审计发现的 6 项 Important 问题（鉴权空 key 绕过、命名空间碰撞、HTTP path 启动 panic、嵌入缓存无界、热路径重复序列化、`search_tools` 文档错标同步），每项 TDD。

**Architecture:** 纯局部修复，不改公开架构：config 校验增强（F2/F3）、mcpgw+downstream 鉴权空值拒绝（F1）、retrieval 缓存改为两代有界（F4）、downstream 字节计量改用 `io::Write` 计数器（F5）、文档同步（F6 + 分层）。不新增第三方 crate。

**Tech Stack:** Rust，std-only 改动，`serde_json::to_writer`，`async_trait`，既有 `config`/`downstream`/`retrieval`/`mcpgw` 测试框架。

> 设计依据：`docs/superpowers/specs/2026-06-15-mcpgw-audit-remediation-design.md`。10 项 Minor 暂缓、不在本计划。

---

## File Structure

| 文件 | 动作 | 整改项 |
|------|------|--------|
| `crates/config/src/lib.rs` | 改 `validate()` + 测试 | F2（server 名前后缀 `_`）、F3（`[server.http].path`） |
| `crates/mcpgw/src/main.rs` | 改 `resolve_api_keys` + 测试 | F1（拒空解析值） |
| `crates/downstream/src/http.rs` | 改 `presented_bearer` | F1（拒空 Bearer token） |
| `crates/downstream/tests/http_server.rs` | 加测试 | F1（空 Bearer → 401） |
| `crates/retrieval/src/caching.rs` | 重写为两代有界缓存 + 测试 | F4 |
| `crates/downstream/src/lib.rs` | 加 `CountingWriter` + 改字节计量 + 测试 | F5 |
| `crates/downstream/Cargo.toml` | 加 `serde`（workspace 既有，无新 crate） | F5 |
| `docs/L4-api/metatools-tools.md`、`docs/L2-components/metatools.md` | 改 | F6 |
| `docs/L3-details/config.md`、`docs/L3-details/retrieval.md`、`docs/L3-details/downstream.md`、`docs/L4-api/downstream-lib.md`、`docs/L3-details/mcpgw-cli.md`、`docs/L1-overview.md` | 改 | F1–F5 分层文档 |

**前置：建分支**

```bash
cd /home/verden/course/mcpgw
git checkout master
git checkout -b fix/audit-remediation
```

---

### Task 1: Config 校验增强（F2 server 名前后缀 `_` + F3 HTTP path）

**Files:**
- Modify: `crates/config/src/lib.rs`（`validate()` + `#[cfg(test)] mod tests`）

- [ ] **Step 1: 写失败测试**

在 `crates/config/src/lib.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn rejects_server_name_leading_or_trailing_underscore() {
    for bad in ["_github", "github_"] {
        let toml = format!(
            "[[upstream]]\nname = \"{bad}\"\ntransport = \"stdio\"\ncommand = \"x\"\n"
        );
        let err = Config::from_toml_str(&toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "server name {bad:?} must be rejected"
        );
    }
}

#[test]
fn accepts_server_name_with_interior_underscore() {
    let cfg = Config::from_toml_str(
        "[[upstream]]\nname = \"my_server\"\ntransport = \"stdio\"\ncommand = \"x\"\n",
    )
    .unwrap();
    assert_eq!(cfg.upstreams[0].name, "my_server");
}

#[test]
fn rejects_invalid_http_path() {
    for bad in ["", "/", "mcp"] {
        let toml = format!("[server.http]\nenabled = true\npath = \"{bad}\"\n");
        let err = Config::from_toml_str(&toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "http path {bad:?} must be rejected"
        );
    }
}

#[test]
fn accepts_default_and_custom_http_path() {
    let cfg = Config::from_toml_str("[server.http]\nenabled = true\n").unwrap();
    assert_eq!(cfg.server.http.unwrap().path, "/mcp");
    let cfg =
        Config::from_toml_str("[server.http]\nenabled = true\npath = \"/gateway\"\n").unwrap();
    assert_eq!(cfg.server.http.unwrap().path, "/gateway");
}
```

- [ ] **Step 2: 跑测试看失败**

Run: `cargo test -p config -- rejects_server_name rejects_invalid_http_path accepts_server_name accepts_default_and_custom`
Expected: `rejects_*` 失败（当前 `validate()` 不拒前后缀 `_` 也不校验 path）。

- [ ] **Step 3: 实现 F2 —— server 名前后缀 `_` 校验**

在 `crates/config/src/lib.rs` 的 `validate()` 中，紧跟现有 `if u.name.contains("__") { ... }` 块**之后**插入：

```rust
            if u.name.starts_with('_') || u.name.ends_with('_') {
                return Err(ConfigError::Invalid(format!(
                    "upstream.name {:?} must not start or end with '_' \
                     (a boundary underscore can re-form the \"__\" namespace separator)",
                    u.name
                )));
            }
```

- [ ] **Step 4: 实现 F3 —— `[server.http].path` 校验**

在 `crates/config/src/lib.rs` 的 `validate()` 中，**`Ok(())` 之前**（即上游 `for` 循环之后）插入：

```rust
        if let Some(http) = &self.server.http {
            if !http.path.starts_with('/') || http.path.len() < 2 {
                return Err(ConfigError::Invalid(format!(
                    "[server.http].path {:?} must start with '/' and be longer than \"/\"",
                    http.path
                )));
            }
        }
```

- [ ] **Step 5: 跑测试看绿**

Run: `cargo test -p config`
Expected: 4 个新测试 PASS，原有 config 测试全 PASS（注意默认 `/mcp` 与既有 `[server.http]` 用例不回归）。

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt -p config && cargo fmt --check -p config && cargo clippy -p config --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 7: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "fix(config): reject boundary-underscore server names + validate http.path (audit F2,F3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 2: 鉴权空密钥加固（F1）

**Files:**
- Modify: `crates/mcpgw/src/main.rs`（`resolve_api_keys` + 单测）
- Modify: `crates/downstream/src/http.rs`（`presented_bearer`）
- Modify: `crates/downstream/tests/http_server.rs`（空 Bearer → 401 测试）

- [ ] **Step 1: 写失败测试（mcpgw 单测 —— 空 env 拒绝）**

在 `crates/mcpgw/src/main.rs` 的 `#[cfg(test)] mod tests` 末尾追加（沿用既有「唯一 env 名 + `set_var`」模式）：

```rust
#[test]
fn resolve_api_keys_rejects_set_but_empty_env() {
    std::env::set_var("MCPGW_AUDIT_EMPTY_KEY", "");
    let cfg = config::Config::from_toml_str(
        "[server.http]\nenabled = true\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_AUDIT_EMPTY_KEY\"\n",
    )
    .unwrap();
    let err = resolve_api_keys(&cfg).unwrap_err();
    assert!(err.contains("empty"), "error must explain the empty secret: {err}");
    assert!(!err.contains("MCPGW_AUDIT_EMPTY_KEY=") , "error must not leak the value");
}
```

- [ ] **Step 2: 写失败测试（downstream http —— 空 Bearer → 401）**

`crates/downstream/tests/http_server.rs` 已有真实 helper：`spawn_http_gateway(state, api_keys) -> String`（返回 URL）与 `post_init(url, bearer: Option<&str>) -> reqwest::StatusCode`（`Some(b)` 发 `Authorization: Bearer {b}`）。把下列测试追加到该文件——它**故意**配置一个空字符串 key 来直击 http 层的纵深防御（非空 key 对空 token 本就会 401，测不出该改动）：

```rust
#[tokio::test]
async fn http_auth_rejects_empty_bearer_even_with_empty_configured_key() {
    // Defense in depth: even if a configured key were empty (resolve_api_keys now prevents this
    // at startup), an empty presented bearer token must NOT authenticate.
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec![String::new()]).await;
    assert_eq!(
        post_init(&url, Some("")).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
}
```

- [ ] **Step 3: 跑测试看失败**

Run: `cargo test -p mcpgw resolve_api_keys_rejects_set_but_empty_env ; cargo test -p downstream --test http_server http_auth_rejects_empty_bearer`
Expected: 两个新测试均失败——空 env 当前被 `resolve_api_keys` 接受；空配置 key + 空 Bearer 当前经 `ct_eq("", b"")==true` 被授权（状态非 401）。

- [ ] **Step 4: 实现 F1 —— `resolve_api_keys` 拒空**

在 `crates/mcpgw/src/main.rs` 的 `resolve_api_keys` 中，把：

```rust
        let secret = std::env::var(&k.env)
            .map_err(|_| format!("api_key {:?}: env {:?} is not set", k.name, k.env))?;
        keys.push(secret);
```

改为：

```rust
        let secret = std::env::var(&k.env)
            .map_err(|_| format!("api_key {:?}: env {:?} is not set", k.name, k.env))?;
        if secret.trim().is_empty() {
            return Err(format!(
                "api_key {:?}: env {:?} is set but empty",
                k.name, k.env
            ));
        }
        keys.push(secret);
```

- [ ] **Step 5: 实现 F1 纵深防御 —— `presented_bearer` 拒空 token**

在 `crates/downstream/src/http.rs` 中，把：

```rust
fn presented_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}
```

改为：

```rust
fn presented_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}
```

- [ ] **Step 6: 跑测试看绿 + 全 crate 回归**

Run: `cargo test -p mcpgw && cargo test -p downstream`
Expected: 两个新测试 PASS；既有鉴权测试（合法 key 通过 / 缺失 / 错误 key → 401）全部不回归。

- [ ] **Step 7: fmt + clippy**

Run: `cargo fmt -p mcpgw -p downstream && cargo fmt --check -p mcpgw -p downstream && cargo clippy -p mcpgw -p downstream --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 8: Commit**

```bash
git add crates/mcpgw/src/main.rs crates/downstream/src/http.rs crates/downstream/tests/http_server.rs
git commit -m "fix(auth): reject empty resolved api-key secret + empty bearer token (audit F1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 3: 两代有界嵌入缓存（F4）

**Files:**
- Modify (rewrite): `crates/retrieval/src/caching.rs`（含 `#[cfg(test)] mod tests`）

- [ ] **Step 1: 重写 `caching.rs` 为两代有界缓存 + 测试**

把 `crates/retrieval/src/caching.rs` 整体替换为下列内容（实现 + 测试一并给出）：

```rust
//! `CachingEmbedder`: an `Embedder` decorator that memoizes vectors by text content hash.
//!
//! Bounded by a two-generation scheme (`current` + `previous`, each capped at
//! `CACHE_GEN_CAP`) so memory cannot grow without bound when arbitrary query texts are
//! embedded. Frequently-seen texts (e.g. tool descriptions re-embedded each rebuild) stay
//! warm via promote-on-hit. Only cache-miss texts are forwarded to `inner`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::embedder::{EmbedError, Embedder};

/// Per-generation entry cap. Total resident entries are bounded by ~`2 * CACHE_GEN_CAP`.
const CACHE_GEN_CAP: usize = 2048;

fn hash_text(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Two-generation bounded cache. Lookups check `current`, then `previous` (promoting a
/// `previous` hit back into `current`). When `current` reaches `CACHE_GEN_CAP`, it rotates into
/// `previous` (dropping the old `previous`) and a fresh `current` starts.
struct GenCache {
    current: HashMap<u64, Arc<[f32]>>,
    previous: HashMap<u64, Arc<[f32]>>,
}

impl GenCache {
    fn new() -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
        }
    }

    /// Look up `key`, promoting a `previous`-generation hit into `current`.
    fn get(&mut self, key: u64) -> Option<Arc<[f32]>> {
        if let Some(v) = self.current.get(&key) {
            return Some(v.clone());
        }
        if let Some(v) = self.previous.remove(&key) {
            self.insert(key, v.clone());
            return Some(v);
        }
        None
    }

    /// Insert `key`, rotating generations first if `current` is full.
    fn insert(&mut self, key: u64, value: Arc<[f32]>) {
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

/// Memoizes embeddings by content hash, bounded by a two-generation cache.
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
        let hashes: Vec<u64> = texts.iter().map(|t| hash_text(t)).collect();

        // First pass: pull cached vectors (promoting previous-gen hits) into a local map and
        // collect the unique misses to embed. The lock is held only for synchronous map ops.
        let mut resolved: HashMap<u64, Arc<[f32]>> = HashMap::new();
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        {
            let mut cache = self.cache.lock().unwrap();
            for (h, t) in hashes.iter().zip(texts) {
                if resolved.contains_key(h) {
                    continue;
                }
                if let Some(v) = cache.get(*h) {
                    resolved.insert(*h, v);
                } else if miss_seen.insert(*h) {
                    miss_texts.push(t.clone());
                }
            }
        }

        // Embed only the misses (skip the call entirely if everything was cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            let mut cache = self.cache.lock().unwrap();
            for (t, v) in miss_texts.iter().zip(embedded) {
                let h = hash_text(t);
                let arc: Arc<[f32]> = Arc::from(v.into_boxed_slice());
                cache.insert(h, arc.clone());
                resolved.insert(h, arc);
            }
        }

        // Reassemble in original input order from the local `resolved` map (NOT the bounded
        // cache, which may have evicted entries within an oversized batch). Every hash is either
        // a first-pass hit or was just embedded+inserted, so the lookup cannot miss — unless the
        // inner embedder returned fewer vectors than inputs (a contract violation the only
        // production `Embedder` rejects as `Err`).
        Ok(hashes
            .iter()
            .map(|h| resolved.get(h).expect("hash resolved above").to_vec())
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

    /// Embedder that counts how many texts it was asked to embed and returns a deterministic
    /// vector derived from each text's hash (so equal texts -> equal vectors).
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
                    v[0] = hash_text(t) as f32;
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
    async fn promote_on_hit_keeps_hot_key_warm_across_churn() {
        let inner = Arc::new(CountingEmbedder::new(2));
        let c = CachingEmbedder::new(inner.clone());
        c.embed(&["hot".into()]).await.unwrap();
        // Churn through >2 full generations of distinct keys, re-touching "hot" within each
        // generation window so promote-on-hit keeps it resident.
        for i in 0..(CACHE_GEN_CAP * 3) {
            c.embed(&[format!("k{i}")]).await.unwrap();
            if i % (CACHE_GEN_CAP / 2) == 0 {
                c.embed(&["hot".into()]).await.unwrap();
            }
        }
        let before = inner.calls.load(Ordering::Relaxed);
        c.embed(&["hot".into()]).await.unwrap();
        assert_eq!(
            inner.calls.load(Ordering::Relaxed),
            before,
            "a periodically-touched hot key must survive churn (no re-embed)"
        );
    }
}
```

- [ ] **Step 2: 跑测试看绿**

Run: `cargo test -p retrieval caching && cargo test -p retrieval`
Expected: 3 个新 caching 测试 PASS；既有 retrieval 测试（含 vector/hybrid/subagent，它们用 `CachingEmbedder`）全部不回归。

- [ ] **Step 3: fmt + clippy**

Run: `cargo fmt -p retrieval && cargo fmt --check -p retrieval && cargo clippy -p retrieval --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 4: Commit**

```bash
git add crates/retrieval/src/caching.rs
git commit -m "fix(retrieval): bound CachingEmbedder with a two-generation cache (audit F4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: `CountingWriter` 字节计量（F5）

**Files:**
- Modify: `crates/downstream/Cargo.toml`（加 `serde`，workspace 既有依赖，无新 crate 进入 lockfile）
- Modify: `crates/downstream/src/lib.rs`（`CountingWriter` + `json_len` + 改 `arg_bytes`/`result_bytes` + 单测）

- [ ] **Step 1: 加 `serde` 依赖到 downstream**

在 `crates/downstream/Cargo.toml` 的 `[dependencies]` 段（`serde_json` 旁）加：

```toml
serde = { workspace = true }
```

> `serde` 已是 workspace 依赖且已在 downstream 的传递依赖图中（经 rmcp/serde_json），此处仅加一行直接依赖，**不引入任何新 crate**。

- [ ] **Step 2: 写失败的单测**

在 `crates/downstream/src/lib.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn json_len_matches_to_string_len() {
    let samples = [
        serde_json::json!({}),
        serde_json::json!({"query": "weather", "top_k": 5}),
        serde_json::json!([1, 2, 3, {"nested": ["a", "b"]}, "unicode: café 日本語"]),
        serde_json::json!("plain string"),
    ];
    for v in samples {
        let expected = serde_json::to_string(&v).unwrap().len();
        assert_eq!(super::json_len(&v), expected, "json_len mismatch for {v}");
    }
}
```

- [ ] **Step 3: 跑测试看失败**

Run: `cargo test -p downstream json_len_matches_to_string_len`
Expected: 编译失败——`json_len` 尚未定义。

- [ ] **Step 4: 实现 `CountingWriter` + `json_len`**

在 `crates/downstream/src/lib.rs` 顶部（`impl` 块之外，靠近其它私有 helper，如 `classify` 附近）加：

```rust
/// A `std::io::Write` that discards bytes and only counts them, so a value's serialized JSON
/// length can be measured without allocating an intermediate `String`.
struct CountingWriter(usize);

impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialized JSON byte length of `value` without allocating a `String` (0 on serialize error).
fn json_len<T: serde::Serialize>(value: &T) -> usize {
    let mut counter = CountingWriter(0);
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => counter.0,
        Err(_) => 0,
    }
}
```

- [ ] **Step 5: 改 `arg_bytes`/`result_bytes` 使用 `json_len`**

在 `crates/downstream/src/lib.rs` 的 `call_tool` 中，把：

```rust
        let arg_bytes = serde_json::to_string(&args).map(|s| s.len()).unwrap_or(0);
```

改为：

```rust
        let arg_bytes = json_len(&args);
```

并把：

```rust
        let result_bytes = match &response {
            Ok(r) => serde_json::to_string(r).map(|s| s.len()).unwrap_or(0),
            Err(_) => 0,
        };
```

改为：

```rust
        let result_bytes = match &response {
            Ok(r) => json_len(r),
            Err(_) => 0,
        };
```

- [ ] **Step 6: 跑测试看绿 + 回归**

Run: `cargo test -p downstream`
Expected: 新 `json_len_matches_to_string_len` PASS；既有埋点测试（`meta_tool_calls_are_observed_with_metadata` 等，断言 `arg_bytes>0`/`result_bytes>0`）全部不回归。

- [ ] **Step 7: fmt + clippy**

Run: `cargo fmt -p downstream && cargo fmt --check -p downstream && cargo clippy -p downstream --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 8: Commit**

```bash
git add crates/downstream/Cargo.toml crates/downstream/src/lib.rs
git commit -m "perf(downstream): count serialized bytes via a length-only writer, not a throwaway String (audit F5)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: 文档同步（F6 + 分层）

docs 必须忠实描述已落地代码。动手前先读对应源码与现有 doc 风格。

**Files:**
- Modify: `docs/L4-api/metatools-tools.md`、`docs/L2-components/metatools.md`（F6）
- Modify: `docs/L3-details/config.md`（F2/F3 校验规则）
- Modify: `docs/L3-details/retrieval.md`（F4 缓存）
- Modify: `docs/L4-api/downstream-lib.md`、`docs/L3-details/downstream.md`（F5 字节计量）
- Modify: `docs/L3-details/mcpgw-cli.md`（F1 鉴权）
- Modify: `docs/L1-overview.md`（测试计数块）

- [ ] **Step 1: F6 —— `search_tools` 标 async**

- `docs/L4-api/metatools-tools.md`：把 `search_tools` 的签名 `pub fn search_tools(...) -> Vec<ToolSummary>` 改为 `pub async fn search_tools(...) -> Vec<ToolSummary>`（与源码 `crates/metatools/src/tools.rs:8` 一致）。
- `docs/L2-components/metatools.md`：接口表里 `search_tools` 行补 `async`（与同表已标 `async` 的 `call_tool` 一致）。

- [ ] **Step 2: F2/F3 —— config 校验规则**

`docs/L3-details/config.md` 的校验小节补两条规则：
- upstream `name`：除既有「非空、不含 `__`、不重复」外，**不得以 `_` 开头或结尾**（边界下划线会重组出 `__` 分隔符，破坏 `{server}__{tool}` 唯一性）。
- `[server.http].path`：必须 `以 "/" 开头且长度 > 1`（拒 `""`、`"/"`、无前导斜杠）——否则启动期 `ConfigError::Invalid`（而非进入 axum 后 panic）。

- [ ] **Step 3: F4 —— 缓存改两代有界**

`docs/L3-details/retrieval.md`（缓存相关小节）把 `CachingEmbedder` 描述从「无界 HashMap、永不淘汰」更新为：
- 两代有界缓存：`current` + `previous`，各上限 `CACHE_GEN_CAP`（2048）；查 `current`→`previous`（命中晋升回 `current`）；`current` 满则轮转（`previous = current`、新 `current`）。
- 内存上界约 `2 * CACHE_GEN_CAP` 条；热文本（如每轮 rebuild 命中的工具描述）经 promote-on-hit 常驻；查询向量不再无界累积。

- [ ] **Step 4: F5 —— 字节计量口径**

- `docs/L4-api/downstream-lib.md` 与 `docs/L3-details/downstream.md`：把 `arg_bytes`/`result_bytes` 的计量从「`serde_json::to_string(...).len()`」更新为「经只计数的 `CountingWriter` + `serde_json::to_writer` 计字节，不分配中间 `String`」；**数值口径不变**。

- [ ] **Step 5: F1 —— 鉴权空值**

`docs/L3-details/mcpgw-cli.md`（及 downstream http 鉴权相关处，如有）注明：
- 启动期 `resolve_api_keys` 拒绝**解析为空/空白**的密钥（fail-fast，消息不含值）。
- http 层 `presented_bearer` 把**空 Bearer token** 视作未提供 → 401（纵深防御）。

- [ ] **Step 6: 更新 L1 测试计数块**

`docs/L1-overview.md` 的测试计数块按实测更新：

```bash
cargo test --all-features 2>&1 | grep "test result:"
```

把新增测试计入（config +4、mcpgw +1、downstream http_server +1、downstream lib +1、retrieval caching +3），重算总数与分项使其相加正确。

- [ ] **Step 7: 校对 + 提交**

- 逐项核对 doc 与真实代码（签名、校验规则、缓存行为、字节口径、鉴权）。
- 提交：

```bash
git add docs/
git commit -m "docs: sync layered docs for audit remediation F1-F6

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 6: 全量验证 + 合回 master

- [ ] **Step 1: 全量验证**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净；clippy 无告警；全测试 PASS（含本次新增 config/mcpgw/downstream/retrieval 测试；`#[ignore]` 真实冒烟仍跳过）。记录总数复核 L1 计数块。

- [ ] **Step 2: 最终整体 code review**

派发最终 whole-feature review（用当前主会话模型），关注：6 项整改是否各自正确且无回归；缓存两代轮转/promote 正确、Mutex 不跨 `.await`；`json_len` 与旧口径数值一致；config 校验不误伤合法配置；鉴权空值在两层均堵死；文档与代码同步。处理 blocking 项，小提交折叠 review nits。

- [ ] **Step 3: 收尾合并**

用 superpowers:finishing-a-development-branch 把 `fix/audit-remediation` 合回 master（`--no-ff`，本地），合并后在 master 复跑 `cargo test --all-features` 确认绿，再删分支。

## 实现期需现场确认/可能回退的点

- F4 `CACHE_GEN_CAP=2048`：如内存预算不同可调；务必保持 Mutex 不跨 `.await`（沿用「锁内取/插、锁外 await embed」结构）。`promote_on_hit_keeps_hot_key_warm_across_churn` 跑 `3*CAP` 次 embed（约 6k，均内存态、快）。
- F5 `serde` 直接依赖：确认 `cargo tree` 不新增 lockfile crate；`json_len` 对 `Err(response)` 分支仍记 `result_bytes=0`（与现状一致）。
- F1 http 测试：用**空字符串配置 key**才能让该测试在修复前失败（非空 key 对空 token 本就 401）。mcpgw 单测用唯一 env 名 + `set_var`（沿用既有模式），断言错误信息不含密钥值。
- F2/F3 文案：仅禁「开头/结尾下划线」，中间下划线（`my_server`）合法；path 校验在 `[server.http]` 段存在时生效（默认 `/mcp` 通过）。
- F6 之外的文档若涉及测试计数，以 `cargo test --all-features` 实测为准。

