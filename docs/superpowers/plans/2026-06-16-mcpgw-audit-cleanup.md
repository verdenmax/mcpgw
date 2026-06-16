# mcpgw 审计收尾整改 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复审计剩余的 5 项 Minor：检索潜在 panic、非环回 bind 无鉴权告警、`gateway-lib.md` 索引误述、`reqwest` dev-dep 版本漂移、`error_kind`/JsonlSink 写错测试缺口。

**Architecture:** 局部硬化，无公开架构变化、**不新增第三方 crate**：retrieval 把违约 panic 改为优雅降级；mcpgw 加启动期安全告警（纯函数可测）；observe 的 writer 循环对 `impl Write` 泛型化以便测自愈；downstream 补 `error_kind` 记录级断言；downstream dev-dep `reqwest` 对齐 0.13；修一处 L4 文档。

**Tech Stack:** Rust，`std::net::SocketAddr`，`std::io::Write`，既有 testkit/CaptureSink/MockUpstream 测试框架。

> 设计依据：`docs/superpowers/specs/2026-06-16-mcpgw-audit-cleanup-design.md`。这是审计 10 项 Minor 的最后 5 项。

---

## File Structure

| 文件 | 动作 | 整改项 |
|------|------|--------|
| `crates/retrieval/src/vector.rs` | 改（空 Ok → BM25）+ 测试 | N1 |
| `crates/retrieval/src/caching.rs` | 改（长度校验 → EmbedError）+ 测试 | N1 |
| `crates/mcpgw/src/main.rs` | 加 `unauthenticated_public_bind` + 告警 + 单测 | N2 |
| `crates/downstream/Cargo.toml` | dev-dep reqwest 0.12→0.13 | N4 |
| `crates/observe/src/audit.rs` | `run_writer` 拆出泛型 `write_loop<W:Write>` + 自愈测试 | N5a |
| `crates/downstream/tests/server.rs` + `tests/common/mod.rs` | 加 timeout/invalid_params 记录测试 + 短超时 mock helper | N5b |
| `docs/L4-api/gateway-lib.md` 等 | 文档同步 | N3 + 分层 |

**前置：建分支**

```bash
cd /home/verden/course/mcpgw
git checkout master
git checkout -b fix/audit-cleanup
```

---

### Task 1: N1 检索潜在 panic → 优雅降级

**Files:**
- Modify: `crates/retrieval/src/vector.rs`（查询路径空 Ok 降级）
- Modify: `crates/retrieval/src/caching.rs`（内层偏短返回 → `EmbedError::Provider`）+ 单测

- [ ] **Step 1: 写失败测试（caching 短返回）**

在 `crates/retrieval/src/caching.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
    /// An embedder that returns FEWER vectors than inputs (a contract violation) must surface as
    /// an `Err`, not a panic in the reassembly step.
    struct ShortEmbedder {
        dim: usize,
    }
    #[async_trait]
    impl Embedder for ShortEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            // One fewer vector than asked for.
            Ok(texts.iter().skip(1).map(|_| vec![0.0f32; self.dim]).collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn short_inner_return_errors_instead_of_panicking() {
        let c = CachingEmbedder::new(Arc::new(ShortEmbedder { dim: 2 }));
        let r = c.embed(&["a".into(), "b".into()]).await;
        assert!(
            matches!(r, Err(EmbedError::Provider(_))),
            "a short inner return must be an Err, not a panic"
        );
    }
```

- [ ] **Step 2: 跑测试看失败**

Run: `cargo test -p retrieval caching short_inner_return`
Expected: panic（`expect("text resolved above")`）而非 `Err`。

- [ ] **Step 3: 实现 caching 长度校验**

在 `crates/retrieval/src/caching.rs` 的 `embed`，把：

```rust
        // Embed only the misses (skip the call entirely if everything was cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            let mut cache = self.cache.lock().unwrap();
```

改为（在拿到 `embedded` 后、加锁插入前，加长度校验）：

```rust
        // Embed only the misses (skip the call entirely if everything was cached).
        if !miss_texts.is_empty() {
            let embedded = self.inner.embed(&miss_texts).await?;
            // Defend the reassembly invariant: a conforming Embedder returns one vector per input.
            // A short/long return would otherwise leave a miss unresolved and panic below.
            if embedded.len() != miss_texts.len() {
                return Err(EmbedError::Provider(format!(
                    "embedder returned {} vectors for {} inputs",
                    embedded.len(),
                    miss_texts.len()
                )));
            }
            let mut cache = self.cache.lock().unwrap();
```

- [ ] **Step 4: 实现 vector 查询空 Ok 降级**

在 `crates/retrieval/src/vector.rs` 的 `search`，把：

```rust
        let qv = match self.embedder.embed(&[query.to_string()]).await {
            Ok(mut v) => normalize(v.remove(0)),
            Err(e) => {
                tracing::warn!(error = %e, "vector query embedding failed; falling back to BM25");
                return self.bm25.search(query, top_k).await;
            }
        };
```

改为：

```rust
        let qv = match self.embedder.embed(&[query.to_string()]).await {
            Ok(mut v) if !v.is_empty() => normalize(v.remove(0)),
            // Empty `Ok` (contract violation) is treated like an error: degrade to BM25 rather
            // than panicking on `v.remove(0)`.
            other => {
                if let Err(e) = other {
                    tracing::warn!(error = %e, "vector query embedding failed; falling back to BM25");
                } else {
                    tracing::warn!("vector query embedding returned no vector; falling back to BM25");
                }
                return self.bm25.search(query, top_k).await;
            }
        };
```

- [ ] **Step 5: 写 vector 空 Ok 降级测试（src 单测，可用 `#[async_trait]`）**

把测试放在 `crates/retrieval/src/vector.rs` 的 `#[cfg(test)] mod tests`（如无则新建；`async_trait` 在 src 内可用，仿 `caching.rs` 的测试 embedder）。追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::RetrievalStrategy;
    use async_trait::async_trait;
    use catalog::{Catalog, ToolDef};
    use serde_json::Value;
    use std::sync::Arc;

    fn tool(server: &str, name: &str, desc: &str) -> ToolDef {
        ToolDef {
            server: server.into(),
            name: name.into(),
            description: desc.into(),
            input_schema: Value::Null,
        }
    }

    /// Indexes fine (multi-element batches → proper vectors) but returns an empty `Ok` for the
    /// single-element query embed — a contract violation that must degrade, not panic.
    struct EmptyOnSingleQuery {
        dim: usize,
    }
    #[async_trait]
    impl Embedder for EmptyOnSingleQuery {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            if texts.len() == 1 {
                Ok(Vec::new())
            } else {
                Ok(texts
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        let mut v = vec![0.0f32; self.dim];
                        v[i % self.dim] = 1.0;
                        v
                    })
                    .collect())
            }
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn search_degrades_to_bm25_on_empty_query_embedding() {
        let catalog = Catalog::from_tooldefs(vec![
            tool("slack", "post_message", "Send a chat message to a Slack channel"),
            tool("weather", "get_forecast", "Get the weather forecast for a location"),
            tool("github", "create_issue", "Create a new issue in a GitHub repository"),
        ]);
        let mut s = VectorStrategy::new(Arc::new(EmptyOnSingleQuery { dim: 64 }));
        s.index(&catalog).await; // 3-element batch -> indexes, not degraded
        // Single-element query embed returns empty Ok -> must fall back to BM25, not panic.
        let hits = s.search("weather forecast location", 3).await;
        assert_eq!(hits[0].qualified_name, "weather__get_forecast");
    }
}
```

> 须确认：`Embedder`/`EmbedError`/`EmptyOnSingleQuery` 与 `vector.rs` 现有 `use`/`normalize` 不冲突（`use super::*` 已带入）；`RetrievalStrategy` trait 需 `use` 以调 `index`/`search`。修复前该测试因 `v.remove(0)` panic、修复后 PASS。`Catalog::from_tooldefs` 与 `ToolDef` 来自 `catalog`（已是 retrieval 依赖）。

- [ ] **Step 6: 跑测试看绿 + 回归**

Run: `cargo test -p retrieval --all-features`
Expected: 新 caching/vector 测试 PASS；既有 retrieval 测试（caching 5、vector、hybrid、subagent）全绿。

- [ ] **Step 7: fmt + clippy**

Run: `cargo fmt -p retrieval && cargo fmt --check -p retrieval && cargo clippy -p retrieval --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 8: Commit**

```bash
git add crates/retrieval/src/vector.rs crates/retrieval/src/caching.rs
git commit -m "fix(retrieval): degrade gracefully on a short/empty embedder return instead of panicking (audit N1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 2: N2 非环回 bind + 空 key 启动期告警

**Files:** Modify `crates/mcpgw/src/main.rs`（`unauthenticated_public_bind` + 告警 + 单测）。

- [ ] **Step 1: 写失败的单测**

在 `crates/mcpgw/src/main.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn unauthenticated_public_bind_flags_only_public_no_key() {
    use super::unauthenticated_public_bind as f;
    assert!(f("0.0.0.0:9000", false), "public bind + no key -> warn");
    assert!(!f("0.0.0.0:9000", true), "public bind WITH key -> ok");
    assert!(!f("127.0.0.1:8970", false), "loopback v4 -> ok");
    assert!(!f("[::1]:9000", false), "loopback v6 -> ok");
    assert!(f("example.com:9000", false), "unparseable host + no key -> conservatively warn");
}
```

- [ ] **Step 2: 跑测试看失败**

Run: `cargo test -p mcpgw unauthenticated_public_bind`
Expected: 编译失败——`unauthenticated_public_bind` 未定义。

- [ ] **Step 3: 实现纯函数**

在 `crates/mcpgw/src/main.rs` 顶部（其它 `fn` 附近，如 `resolve_api_keys` 旁）加：

```rust
/// True when an HTTP server with NO api keys is bound to a non-loopback (public) address — an
/// unauthenticated public exposure worth a loud warning. Unparseable binds (e.g. `host:port`)
/// can't be proven loopback, so they warn conservatively.
fn unauthenticated_public_bind(bind: &str, has_keys: bool) -> bool {
    if has_keys {
        return false;
    }
    match bind.parse::<std::net::SocketAddr>() {
        Ok(addr) => !addr.ip().is_loopback(),
        Err(_) => true,
    }
}
```

- [ ] **Step 4: 接线告警**

在 `crates/mcpgw/src/main.rs` 的 HTTP 装配处（紧邻现有 `tracing::info!(... "http server listening")`），在绑定后加：

```rust
        if unauthenticated_public_bind(&h.bind, !api_keys.is_empty()) {
            tracing::warn!(
                bind = %h.bind,
                "HTTP server is UNAUTHENTICATED and bound to a non-loopback address; \
                 configure [[server.http.api_key]] or bind to localhost"
            );
        }
```

> 注：此处 `api_keys` 已在前文 `resolve_api_keys` 得到；`h` 是 `cfg.server.http` 的引用，`h.bind` 是绑定串。按 `main.rs` 现有变量名接线（若 `api_keys` 已被 move 进 router，则在 move 之前先算 `let has_keys = !api_keys.is_empty();` 并用之）。

- [ ] **Step 5: 跑测试 + 回归**

Run: `cargo test -p mcpgw --all-features`
Expected: 新单测 PASS；既有 mcpgw 测试（audit e2e、cli、resolve_api_keys 等）全绿。

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt -p mcpgw && cargo fmt --check -p mcpgw && cargo clippy -p mcpgw --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 7: Commit**

```bash
git add crates/mcpgw/src/main.rs
git commit -m "fix(mcpgw): loudly warn on a non-loopback HTTP bind with no api keys (audit N2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 3: N4 `reqwest` dev-dep 对齐 0.13

**Files:** Modify `crates/downstream/Cargo.toml`（+ `Cargo.lock` 去重副作用）。

- [ ] **Step 1: 改 dev-dep**

在 `crates/downstream/Cargo.toml` 的 `[dev-dependencies]`，把：

```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
```

改为（与 `chat`/`embedder` 一致；0.13 的 rustls feature 名为 `"rustls"`，非 `"rustls-tls"`）：

```toml
reqwest = { version = "0.13", default-features = false, features = ["rustls"] }
```

- [ ] **Step 2: 跑 downstream 测试（http_server.rs 用 reqwest）**

Run: `cargo test -p downstream --all-features`
Expected: 全绿——`crates/downstream/tests/http_server.rs` 的 `reqwest::Client::new()/.post()/.header()/.body()/.send()/.status()` 与 `reqwest::StatusCode` 在 0.12→0.13 间 API 不变。若有任何 API 差异，做最小测试代码调整。

- [ ] **Step 3: 验证去重**

Run: `cargo tree -p downstream --all-features --duplicates 2>/dev/null | grep -E "^reqwest v0|reqwest v0" | sort -u`
Expected: 不再出现两个 reqwest 大版本（0.12 与 0.13 并存消失）。

Run: `git diff --stat Cargo.lock`
Expected: `Cargo.lock` 仅为去重/版本变动；**不应新增第三方 crate 包条目**（确认 `git show` 无新 `[[package]]` 是某全新 crate；reqwest 0.12 相关被移除属预期）。

- [ ] **Step 4: fmt 检查（Cargo.toml 无需 fmt，仅确认 build）**

Run: `cargo build --locked -p downstream --all-features`
Expected: lockfile 一致、构建成功。

- [ ] **Step 5: Commit**

```bash
git add crates/downstream/Cargo.toml Cargo.lock
git commit -m "build(downstream): align dev-dep reqwest to 0.13 (rustls), dedupe the http stack (audit N4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: N5a JsonlSink 写错自愈测试（writer 泛型化）

**Files:** Modify `crates/observe/src/audit.rs`（拆出 `write_loop<W: Write>` + 自愈测试）。

- [ ] **Step 1: 重构 `run_writer`，拆出泛型 `write_loop` + `write_line` 泛型化**

在 `crates/observe/src/audit.rs`，把现有：

```rust
fn run_writer(rx: Receiver<String>, file: File) {
    let mut w = BufWriter::new(file);
    let mut write_errors: u64 = 0;
    while let Ok(line) = rx.recv() {
        write_line(&mut w, &line, &mut write_errors);
        while let Ok(more) = rx.try_recv() {
            write_line(&mut w, &more, &mut write_errors);
        }
        if let Err(e) = w.flush() {
            rate_limited_write_error(&mut write_errors, &e);
        }
    }
    if let Err(e) = w.flush() {
        rate_limited_write_error(&mut write_errors, &e);
    }
    if let Ok(file) = w.into_inner() {
        let _ = file.sync_all();
    }
}

fn write_line(w: &mut BufWriter<File>, line: &str, errors: &mut u64) {
    if let Err(e) = w
        .write_all(line.as_bytes())
        .and_then(|_| w.write_all(b"\n"))
    {
        rate_limited_write_error(errors, &e);
    }
}
```

替换为（把 append/flush 循环抽成对 `impl Write` 泛型的 `write_loop`；`fsync` 仍仅对 `File`）：

```rust
fn run_writer(rx: Receiver<String>, file: File) {
    let mut w = BufWriter::new(file);
    write_loop(&rx, &mut w);
    // fsync is File-specific: only the production path commits to stable storage.
    if let Ok(file) = w.into_inner() {
        let _ = file.sync_all();
    }
}

/// The append-and-flush loop, generic over the sink so the write-error self-heal is testable.
/// Returns when the channel disconnects (all senders dropped). Write/flush errors only
/// rate-limit-warn and never stop the loop — transient faults self-heal.
fn write_loop<W: Write>(rx: &Receiver<String>, w: &mut W) {
    let mut write_errors: u64 = 0;
    while let Ok(line) = rx.recv() {
        write_line(w, &line, &mut write_errors);
        while let Ok(more) = rx.try_recv() {
            write_line(w, &more, &mut write_errors);
        }
        if let Err(e) = w.flush() {
            rate_limited_write_error(&mut write_errors, &e);
        }
    }
    if let Err(e) = w.flush() {
        rate_limited_write_error(&mut write_errors, &e);
    }
}

fn write_line<W: Write>(w: &mut W, line: &str, errors: &mut u64) {
    if let Err(e) = w
        .write_all(line.as_bytes())
        .and_then(|_| w.write_all(b"\n"))
    {
        rate_limited_write_error(errors, &e);
    }
}
```

> `BufWriter<File>` 实现 `Write`，故 `run_writer` 仍可把 `&mut BufWriter<File>` 传给 `write_loop`。生产路径（append/flush/`sync_all`）行为零变化。

- [ ] **Step 2: 写自愈测试**

在 `crates/observe/src/audit.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
    /// A `Write` that fails its first `fail_first` writes, then succeeds — to exercise the
    /// writer's keep-alive-on-error self-heal.
    struct FlakyWriter {
        fail_first: usize,
        writes: usize,
        data: Vec<u8>,
    }
    impl std::io::Write for FlakyWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            if self.writes <= self.fail_first {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "transient write error"));
            }
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_loop_self_heals_after_write_errors() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(8);
        for i in 0..4 {
            tx.send(format!("line{i}")).unwrap();
        }
        drop(tx); // disconnect so write_loop drains and returns

        // Fail the first 3 writes (line0..line2 lost), then succeed: line3 must still land —
        // proving the loop kept going rather than exiting on the first error.
        let mut w = FlakyWriter {
            fail_first: 3,
            writes: 0,
            data: Vec::new(),
        };
        super::write_loop(&rx, &mut w);

        let out = String::from_utf8(w.data).unwrap();
        assert!(
            out.contains("line3"),
            "writer must self-heal and keep writing after errors; got {out:?}"
        );
    }
```

- [ ] **Step 3: 跑测试看绿 + 回归**

Run: `cargo test -p observe --all-features`
Expected: 新 `write_loop_self_heals_after_write_errors` PASS；既有 observe 测试（含 audit 写盘/drain/open-failure、capture）全绿。

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt -p observe && cargo fmt --check -p observe && cargo clippy -p observe --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 5: Commit**

```bash
git add crates/observe/src/audit.rs
git commit -m "test(observe): cover the JsonlSink writer's write-error self-heal via a generic write_loop (audit N5a)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: N5b `error_kind` 记录级断言（timeout + invalid_params）

**Files:**
- Modify: `crates/downstream/tests/common/mod.rs`（短超时 mock helper）
- Modify: `crates/downstream/tests/server.rs`（两条记录测试）

- [ ] **Step 1: 加短超时 mock helper**

在 `crates/downstream/tests/common/mod.rs` 的 `attach_mock` 之后追加（仿其结构，套用 `with_call_timeout`）：

```rust
/// Like `attach_mock` but with a short per-call timeout, so calling the `slow` tool reliably
/// trips `MetaError::Timeout`.
pub async fn attach_mock_with_timeout(
    state: &GatewayState,
    name: &str,
    timeout: std::time::Duration,
) {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        let svc = MockUpstream::new()
            .serve(server_io)
            .await
            .expect("mock upstream serves");
        let _ = svc.waiting().await;
    });
    let handle = UpstreamHandle::connect(name, client_io)
        .await
        .unwrap()
        .with_call_timeout(timeout);
    state.registry().insert(std::sync::Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}
```

- [ ] **Step 2: 写两条记录测试**

在 `crates/downstream/tests/server.rs` 追加（仿现有 `upstream_tool_error_is_recorded_as_error_outcome` 的 setup）：

```rust
#[tokio::test]
async fn timeout_call_is_recorded_with_timeout_error_kind() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock_with_timeout(&state, "mock", std::time::Duration::from_millis(50)).await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    // `slow` sleeps ~10s server-side; the 50ms call timeout trips MetaError::Timeout.
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name": "mock__slow"}))),
        )
        .await
        .unwrap();
    client.cancel().await.unwrap();

    let recs = cap.records();
    let rec = recs.last().expect("a record for the call");
    assert_eq!(rec.meta_tool, MetaTool::CallTool);
    assert_eq!(rec.outcome, CallOutcome::Timeout);
    assert_eq!(rec.error_kind, Some("timeout"));
}

#[tokio::test]
async fn missing_name_is_recorded_with_invalid_params_error_kind() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    // call_tool with no "name" -> the invalid_params arm.
    let _ = client
        .call_tool(CallToolRequestParams::new("call_tool").with_arguments(args(json!({}))))
        .await
        .unwrap();
    client.cancel().await.unwrap();

    let recs = cap.records();
    let rec = recs.last().expect("a record for the call");
    assert_eq!(rec.meta_tool, MetaTool::CallTool);
    assert_eq!(rec.outcome, CallOutcome::Error);
    assert_eq!(rec.error_kind, Some("invalid_params"));
}
```

> `upstream_unavailable`/`upstream_call`/`internal` 难以经公开 API 确定性触发，**不**新增记录级测试（如实保留覆盖局限）。

- [ ] **Step 3: 跑测试看绿**

Run: `cargo test -p downstream --all-features`
Expected: 两条新测试 PASS；既有 downstream 测试全绿。

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt -p downstream && cargo fmt --check -p downstream && cargo clippy -p downstream --all-targets --all-features -- -D warnings`
Expected: 干净、无告警。

- [ ] **Step 5: Commit**

```bash
git add crates/downstream/tests/common/mod.rs crates/downstream/tests/server.rs
git commit -m "test(downstream): record-level assertions for timeout + invalid_params error_kind (audit N5b)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 6: 文档同步（N3 + N1/N2/N5a 分层 + L1 计数）

**Files:**
- Modify: `docs/L4-api/gateway-lib.md`（N3）
- Modify: `docs/L3-details/retrieval.md`（N1）、`docs/L3-details/mcpgw-cli.md`（N2）、`docs/L4-api/observe-audit.md`（N5a）
- Modify: `docs/L1-overview.md`（测试计数块）

- [ ] **Step 1: N3 修正 gateway-lib.md**

`docs/L4-api/gateway-lib.md`：删除/改写「`GatewayState::new` 用 `build_strategy` 新建策略、对空 `Catalog` `index`」中的「对空 `Catalog` `index`」一句——构造时**不**索引（仅装入空快照；首次 `rebuild_snapshot` 内才 `index`），与 `docs/L1-overview.md` 一致。

- [ ] **Step 2: N1 检索降级**

`docs/L3-details/retrieval.md`（vector/caching 小节）：补「查询 embedder 返回空 `Ok` → vector 降级 BM25（不再 `remove(0)` panic）；inner 返回向量数 ≠ 输入数 → `CachingEmbedder` 返回 `EmbedError::Provider`（上层据此降级），重组不再 `expect` panic」。

- [ ] **Step 3: N2 安全告警**

`docs/L3-details/mcpgw-cli.md`（serve 鉴权小节）：补「绑定非环回地址且无 `api_key` 时，启动期发显著 `warn`（不阻断），提示服务无鉴权」。

- [ ] **Step 4: N5a writer 泛型**

`docs/L4-api/observe-audit.md`：把 writer 描述更新为「append/flush 循环抽为对 `impl Write` 泛型的 `write_loop`（便于测写错自愈）；`fsync`（`sync_all`）仍仅生产 `File` 路径」。

- [ ] **Step 5: L1 测试计数**

`docs/L1-overview.md` 测试计数块按实测更新：

```bash
cargo test --all-features 2>&1 | grep "test result:"
```

把新增测试计入（retrieval caching +1、retrieval vector +1、mcpgw +1、observe +1、downstream server +2），重算总数与分项使其相加正确。

- [ ] **Step 6: 校对 + 提交**

- 逐项核对 doc 与真实代码（gateway 构造不索引、检索降级、安全告警、writer 泛型）。
- 确认无产品文档仍称「`GatewayState::new` 构造时 index」。

```bash
git add docs/
git commit -m "docs: gateway-lib index fix + sync N1/N2/N5a layered docs + test count (audit N3 + cleanup)

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
Expected: fmt 干净；clippy 无告警；全测试 PASS（含本次新增 retrieval/mcpgw/observe/downstream 测试）；lockfile 一致。记录总数复核 L1。

- [ ] **Step 2: 最终整体 code review**

派发最终 whole-feature review（用当前主会话模型），关注：N1 两处降级正确且不破坏 `Embedder` 契约、空 Ok/短返回均不再 panic；N2 `unauthenticated_public_bind` 判定全面（v4/v6 环回、unspecified、主机名）、告警仅警告不阻断；N4 去重生效且无新 crate；N5a 泛型化未改生产行为、自愈测试真实；N5b 两条记录测试稳定（短超时不 flaky）；文档同步。处理 blocking 项，小提交折叠 nits。

- [ ] **Step 3: 收尾合并**

用 superpowers:finishing-a-development-branch 把 `fix/audit-cleanup` 合回 master（`--no-ff`，本地），合并后在 master 复跑 `cargo test --all-features` 确认绿，再删分支。**至此审计 16 项发现（0 Critical / 6 Important / 10 Minor）全部清零。**

## 实现期需现场确认/可能回退的点

- N1 vector 测试：构造 `VectorStrategy` 以 `tests/vector.rs` 现状为准；务必真实触达空-Ok 降级路径（修复前 panic、修复后 BM25）。
- N2 告警接线：`api_keys` 若已被 move 进 router，先 `let has_keys = !api_keys.is_empty();`。
- N4：0.13 的 rustls feature 名为 `"rustls"`（非 `"rustls-tls"`）；确认 `cargo tree --duplicates` 去重且 lockfile 不新增 crate。
- N5a：只把循环体泛型化、`fsync` 留 `File`；`spawn_writer` 生产行为零变化。
- N5b：`slow` 睡 10s、超时设 50ms 稳定触发 `timeout`；`with_call_timeout` 是 `UpstreamHandle` 既有 API。

