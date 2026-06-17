# mcpgw dashboard 审计修复（pass-1）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 dashboard 子系统审计的 2 项 Minor：N1 给无鉴权 localhost 面板加 Host 头校验（仅 loopback 绑定时）防 DNS rebinding；N2 截断 discovery 追踪里的 query 长度以封顶内存。

**Architecture:** N2 在 `downstream` 构造 `DiscoveryRecord` 处把 query 截到 2048 字符（UTF-8 安全）。N1 在 `dashboard` crate 加一个可单测的纯函数 `host_is_local` + 一层 axum 中间件，`build_dashboard_router` 新增 `enforce_loopback_host: bool`，由 `mcpgw serve` 按 bind 是否 loopback 计算传入。

**Tech Stack:** Rust workspace、axum 0.8（`middleware::from_fn`）、tokio。

参考 spec：`docs/superpowers/specs/2026-06-17-mcpgw-dashboard-audit-fixes-design.md`。

**全局门禁（每个实现 task 后都要过）：** `cargo fmt --all --check` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --all-features`；提交信息末尾加 `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`。

---

## File Structure

| 文件 | 职责 | 本计划 |
|---|---|---|
| `crates/downstream/src/lib.rs` | search_tools 捕获 + `discovery_record_for_search` | N2：`MAX_TRACE_QUERY_CHARS` + `clamp_query` + 用在 helper + 测试（Task 1） |
| `crates/dashboard/src/lib.rs` | router + handlers | N1：`host_is_local` 纯函数 + `require_local_host` 中间件 + `build_dashboard_router` 加 `enforce_loopback_host` 参 + 测试（Task 2） |
| `crates/mcpgw/src/main.rs` | serve 装配 | N1：计算 `enforce_loopback_host` 并传入 `build_dashboard_router`（Task 2） |
| `docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`、`docs/L4-api/mcpgw-main.md`、`docs/L1-overview.md` | 分层文档 + 计数 | Task 3 |

---

## Task 1: N2 — 截断 discovery 追踪里的 query 长度（downstream）

**Files:**
- Modify: `crates/downstream/src/lib.rs`（`discovery_record_for_search` 附近：加常量 + `clamp_query` + 改 `query` 构造 + 测试）
- Test: `crates/downstream/src/lib.rs` 的 `#[cfg(test)] mod tests`

背景：discovery ring 只限条数（`trace_buffer`），逐条存完整 client query，故反复发超大 query（需开 `trace_queries`）放大内存。把 query 截到 2048 字符即封顶 ring 内存。

- [ ] **Step 1: 写失败测试**

在 `crates/downstream/src/lib.rs` 的 `mod tests` 内，紧跟现有 `discovery_record_maps_query_and_scored_hits` 之后加：

```rust
    #[test]
    fn discovery_query_is_clamped_to_the_cap() {
        let long = "x".repeat(MAX_TRACE_QUERY_CHARS + 100);
        let rec = discovery_record_for_search(&long, 1, &[], 0);
        assert_eq!(rec.query.chars().count(), MAX_TRACE_QUERY_CHARS);
        let rec2 = discovery_record_for_search("hello", 1, &[], 0);
        assert_eq!(rec2.query, "hello", "short query is unchanged");
    }

    #[test]
    fn discovery_query_clamp_is_utf8_safe() {
        // Multi-byte chars near the boundary must not split a code point.
        let q: String = "é".repeat(MAX_TRACE_QUERY_CHARS + 10);
        let rec = discovery_record_for_search(&q, 1, &[], 0);
        assert_eq!(rec.query.chars().count(), MAX_TRACE_QUERY_CHARS);
        assert!(rec.query.chars().all(|c| c == 'é'), "no split code point");
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p downstream discovery_query 2>&1 | tail -20`
Expected: FAIL —— `MAX_TRACE_QUERY_CHARS` 未定义（编译错误）。

- [ ] **Step 3: 最小实现**

在 `crates/downstream/src/lib.rs` 的 `discovery_record_for_search` 函数**之前**加常量与 helper：

```rust
/// Max characters of a client query retained in a discovery trace. Bounds the discovery ring's
/// resident memory to `trace_buffer * MAX_TRACE_QUERY_CHARS` rather than by client input size.
const MAX_TRACE_QUERY_CHARS: usize = 2048;

/// Truncate `query` to at most `MAX_TRACE_QUERY_CHARS` characters (operates on `char`s, so it is
/// UTF-8 safe and never splits a code point).
fn clamp_query(query: &str) -> String {
    query.chars().take(MAX_TRACE_QUERY_CHARS).collect()
}
```

把 `discovery_record_for_search` 里的 `query: query.to_string(),` 改为：

```rust
        query: clamp_query(query),
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p downstream`
Expected: PASS —— 2 个新测试 + 现有 downstream 测试全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p downstream --all-targets --all-features -- -D warnings && cargo test -p downstream
git add crates/downstream/src/lib.rs
git commit -m "fix(downstream): clamp discovery-trace query length (audit N2)

The discovery ring caps entry count (trace_buffer) but stored the full client
query verbatim, so a flood of huge queries (with trace_queries on) amplified
memory. Truncate the captured query to MAX_TRACE_QUERY_CHARS (UTF-8 safe) so the
ring's resident memory is bounded.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: N1 — Host 头校验防 DNS rebinding（dashboard + mcpgw 装配）

**Files:**
- Modify: `crates/dashboard/src/lib.rs`（`host_is_local` 纯函数 + `require_local_host` 中间件 + `build_dashboard_router` 加 `enforce_loopback_host` 参 + 测试）
- Modify: `crates/mcpgw/src/main.rs`（计算 `enforce_loopback_host` 传入 `build_dashboard_router`）
- Test: `crates/dashboard/src/lib.rs` 的 `#[cfg(test)] mod` 测试

背景：dashboard 无鉴权、安全模型是「绑 loopback 故只本机可达」，但无 Host 头校验 → DNS rebinding 可同源读 `/api/*`。仅在 loopback 绑定时校验 Host（非 loopback 是运维显式暴露，跳过校验不破坏）。

- [ ] **Step 1: 写失败测试**

在 `crates/dashboard/src/lib.rs` 末尾加一个测试模块：

```rust
#[cfg(test)]
mod host_tests {
    use super::host_is_local;

    #[test]
    fn host_is_local_accepts_loopback_rejects_remote() {
        for ok in ["127.0.0.1:8971", "127.0.0.1", "localhost", "localhost:8971",
                   "LOCALHOST:8971", "[::1]:8971", "[::1]", "127.0.0.5:8971"] {
            assert!(host_is_local(Some(ok)), "{ok} should be local");
        }
        for bad in ["evil.com:8971", "192.168.1.5:8971", "example.com", "0.0.0.0:8971", "[::]:8971"] {
            assert!(!host_is_local(Some(bad)), "{bad} should NOT be local");
        }
        assert!(!host_is_local(None), "missing Host -> not local");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p dashboard host_is_local 2>&1 | tail -20`
Expected: FAIL —— `host_is_local` 未定义。

- [ ] **Step 3: 最小实现（dashboard）**

在 `crates/dashboard/src/lib.rs` 顶部 `use` 区补：

```rust
use axum::extract::Request;
use axum::http::header::HOST;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
```

加纯函数 + 中间件（放在 `build_dashboard_router` 之前）：

```rust
/// True if the `Host` header names the local machine (the literal `localhost`, or an IP that is a
/// loopback address). Defends the unauthenticated dashboard against DNS rebinding when bound to
/// loopback: a remote page that rebinds its hostname to 127.0.0.1 still sends its OWN hostname in
/// `Host`, which is rejected. Missing/unparseable Host -> not local.
fn host_is_local(host: Option<&str>) -> bool {
    let Some(raw) = host else {
        return false;
    };
    // Strip the optional port. IPv6 hosts are bracketed in `Host`: `[::1]:8971` / `[::1]`.
    let host = if let Some(rest) = raw.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, _)) => inner,
            None => return false,
        }
    } else {
        raw.rsplit_once(':').map_or(raw, |(h, _)| h)
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Reject (403) any request whose `Host` is not local. Mounted only when the dashboard is bound to
/// loopback (see `build_dashboard_router`).
async fn require_local_host(req: Request, next: Next) -> axum::response::Response {
    let host = req.headers().get(HOST).and_then(|v| v.to_str().ok());
    if host_is_local(host) {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}
```

把 `build_dashboard_router` 签名与返回改为带可选中间件：

```rust
/// Build the dashboard's router. When `enforce_loopback_host` is true (dashboard bound to
/// loopback), a layer rejects requests whose `Host` isn't local, closing the DNS-rebinding vector.
pub fn build_dashboard_router(state: Arc<AppState>, enforce_loopback_host: bool) -> axum::Router {
    let router = axum::Router::new()
        .route("/api/overview", get(h_overview))
        .route("/api/upstreams", get(h_upstreams))
        .route("/api/tools", get(h_tools))
        .route("/api/metrics", get(h_metrics))
        .route("/api/traces", get(h_traces))
        .route("/api/metrics/history", get(h_metrics_history))
        .route("/", get(h_index))
        .route("/app.js", get(h_app_js))
        .route("/style.css", get(h_style_css))
        .with_state(state);
    if enforce_loopback_host {
        router.layer(middleware::from_fn(require_local_host))
    } else {
        router
    }
}
```

（即在现有 `build_dashboard_router` 的 `.with_state(state)` 之后，把直接返回改为按 `enforce_loopback_host` 决定是否 `.layer(...)`。）

- [ ] **Step 4: 装配传参（mcpgw，保持工作区绿）**

在 `crates/mcpgw/src/main.rs` 把 `let router = dashboard::build_dashboard_router(app_state);` 改为：

```rust
        // Enforce a local Host header only when bound to loopback (non-loopback is an explicit,
        // already-warned operator exposure that they front themselves).
        let enforce_loopback_host = !unauthenticated_public_bind(&cfg.dashboard.bind, false);
        let router = dashboard::build_dashboard_router(app_state, enforce_loopback_host);
```

- [ ] **Step 5: 运行测试确认通过（含 e2e）**

Run:
```bash
cargo test -p dashboard
cargo build --all-targets
cargo test -p mcpgw --test dashboard -- --ignored
```
Expected: PASS —— `host_is_local` 测试 + 现有 dashboard 测试全绿；工作区编译通过；ignored e2e 仍 PASS（reqwest 发 `Host: 127.0.0.1:<port>`，loopback 绑定下校验放行）。

- [ ] **Step 6: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
git add crates/dashboard/src/lib.rs crates/mcpgw/src/main.rs
git commit -m "fix(dashboard): reject non-local Host on a loopback bind (audit N1, anti-DNS-rebinding)

The unauthenticated dashboard relied on loopback binding for access control, but
without a Host check a DNS-rebinding page could same-origin fetch /api/*. When
bound to loopback, reject (403) any request whose Host isn't localhost/a loopback
IP; non-loopback binds (explicit operator exposure) skip the check.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: 文档同步 + 测试计数 + 验证 + 合并

**Files:**
- Modify: `docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`、`docs/L4-api/mcpgw-main.md`、`docs/L1-overview.md`

- [ ] **Step 1: 文档同步**

- `docs/L3-details/dashboard.md`（进程模型/安全段）+ `docs/L4-api/dashboard.md`（`build_dashboard_router`）：记录新增的 **Host 头校验**——仅当 bind 为 loopback 时挂中间件，非 local Host → 403，防 DNS rebinding；非 loopback 绑定跳过。`build_dashboard_router` 签名新增 `enforce_loopback_host: bool`。
- `docs/L4-api/dashboard.md`（或 downstream/observe 相关）：记录 discovery 追踪的 query 截断（`MAX_TRACE_QUERY_CHARS = 2048`，UTF-8 安全）；指明 ring 内存上限 = `trace_buffer × 2048 字符`。
- `docs/L4-api/mcpgw-main.md`：装配处计算 `enforce_loopback_host = !unauthenticated_public_bind(bind, false)` 并传入。

- [ ] **Step 2: L1 测试计数**

Run: `cargo test --all-features 2>&1 | grep "test result:"`
求和更新 `docs/L1-overview.md` 测试计数块：本计划新增 downstream +2、dashboard +1 → 约 225 → **228 passed**；ignored 不变（4）。以实跑为准，分项要能求和。

- [ ] **Step 3: 提交文档**

```bash
git add docs/
git commit -m "docs: sync dashboard docs for Host-header check + query clamp + test count

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

- [ ] **Step 4: 全门禁复跑 + 最终整分支审查**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features 2>&1 | grep "test result:"
cargo test -p mcpgw --test dashboard -- --ignored
cargo build --locked
```
以 `git merge-base master <branch>` 为基跑 `code-review` 子代理（model `claude-opus-4.8`），折叠 Critical/Important 项。

- [ ] **Step 5: 合并 + 推送（finishing-a-development-branch）**

征得用户确认后 `--no-ff` 合并入 master、master 复测全绿、删分支、`git push origin master`；findings id 20/21 置 `fixed`；向用户用中文汇报。

---

## Self-Review（plan 作者自查）

- **Spec coverage**：N1 Host 校验 → Task 2（纯函数 + 中间件 + 装配）；N2 query 截断 → Task 1；文档+计数+验证+合并 → Task 3。spec「不做的事」未越界（无鉴权、无 allowed_hosts、不截工具名、无新依赖）。✓
- **Placeholder scan**：每步含完整代码与确切命令；无 TBD/TODO。✓
- **Type/名一致**：`host_is_local`/`require_local_host`/`build_dashboard_router(state, enforce_loopback_host)`/`clamp_query`/`MAX_TRACE_QUERY_CHARS`/`discovery_record_for_search` 全程一致；`build_dashboard_router` 的新参在 Task 2 引入且唯一调用处（mcpgw main.rs）同 task 更新，保持每步绿。`unauthenticated_public_bind` 为现有 helper（main.rs），loopback 绑定时返回 false → `enforce=true`。✓
