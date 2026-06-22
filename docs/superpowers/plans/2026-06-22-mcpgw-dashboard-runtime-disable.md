# Dashboard 运行时临时禁用 + Bearer 鉴权 实施计划（M4 写子系统 B）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 dashboard 加运行时临时禁用上游/工具的能力（隐藏式语义、跨重启持久化、Bearer 鉴权写、开放只读），且默认行为完全不变。

**Architecture:** `DisableSet`（有界内存 + 可选 JSON 状态文件、原子写）落在 `gateway` crate，作为 `GatewayState` 的新字段；`rebuild_snapshot` 读它跳过被禁用上游的 ingest 与被禁用单工具的 upsert——隐藏式语义经现有 metatools/downstream 代码路径自然达成（零改动）。dashboard 暴露开放的 `GET /api/disabled` 与 Bearer 鉴权的 `POST /api/admin/*`（复用 `downstream/src/http.rs` 的 `subtle` 常量时间比较模式），handler 改集 → 持久化 → `await gateway.rebuild_snapshot()`。两个独立 opt-in 配置开关解耦"读应用"与"写鉴权"。

**Tech Stack:** Rust（axum 0.8 / tokio / serde / serde_json / subtle 2 / arc-swap）+ Svelte 5 + Vite（dashboard UI，dist 入库经 rust-embed 内嵌）。

**Spec:** `docs/superpowers/specs/2026-06-22-mcpgw-dashboard-runtime-disable-design.md`

---

## 文件结构（创建/修改一览）

**`gateway` crate**
- 创建 `crates/gateway/src/disable.rs` — `DisableSet` / `DisabledState` / `DisabledSnapshot` + 加载/原子落盘 + 单元测试。
- 修改 `crates/gateway/Cargo.toml` — 加 `serde`(derive) + `serde_json` 依赖。
- 修改 `crates/gateway/src/lib.rs` — `mod disable` + 重导出；`GatewayState.disabled` 字段 + `with_disabled`/`disabled()`；`rebuild_snapshot` 过滤。
- 创建 `crates/gateway/tests/disable.rs` — MockUpstream 集成测试（禁用上游/工具 → 搜索消失 → 启用恢复）。

**`config` crate**
- 修改 `crates/config/src/lib.rs` — `DashboardConfig` 加 `admin_token_env` / `disabled_state_path`（皆 `Option<String>`）+ Default + 测试。

**`dashboard` crate**
- 创建 `crates/dashboard/src/admin.rs` — `presented_bearer` + `authorize` 纯决策 + `require_admin_token` 中间件 + 4 个 disable/enable handler。
- 修改 `crates/dashboard/Cargo.toml` — 加 `subtle = "2"`。
- 修改 `crates/dashboard/src/api.rs` — `AppState` 加 `admin_token: Option<Arc<str>>`；`disabled()` 纯函数。
- 修改 `crates/dashboard/src/lib.rs` — `mod admin`；`h_disabled` 包装；挂 `GET /api/disabled` + admin 子路由（route_layer 鉴权中间件）。
- 修改 `crates/dashboard/src/about.rs` — `DashboardInfo` 加 `admin_enabled: bool` + 隐私测试。

**`mcpgw` bin**
- 修改 `crates/mcpgw/src/main.rs` — `resolve_admin_token`（fail-fast）；加载 `DisableSet` 经 `with_disabled` 注入（首次 rebuild 前）；AppState 设 `admin_token`。

**UI（Svelte 5）**
- 创建 `crates/dashboard/ui/src/lib/admin.svelte.js` — 内存 token 态 + `adminPost`。
- 创建 `crates/dashboard/ui/src/lib/DisableToggle.svelte` — 复用的禁用/启用按钮（仅 token 在时显示）。
- 修改 `crates/dashboard/ui/src/lib/api.js` — 加 `postJSON`。
- 修改 `About.svelte` / `Upstreams.svelte` / `UpstreamDetail.svelte` / `Tools.svelte` / `ToolDetail.svelte` — token 输入 + 禁用徽标 + 启停开关 + Disabled tools 区。
- 修改 `crates/dashboard/ui/src/app.css` — 加 `.admbtn` 样式。
- 重建 `crates/dashboard/ui/dist/*`（`npm run build`，入库）。

**文档**：L1-overview、L2/L3 dashboard、L2/L3 gateway、L2/L3/L4 config、L4 dashboard、L4 mcpgw-main 同步。

**`.gitignore`**：加 `mcpgw-disabled.json`（运行时状态文件不入库）。

## 任务总览（11 个，逐个 spec+质量双重审查）

1. `DisableSet` 核心 + 持久化（gateway，纯单元可测）
2. `GatewayState` 接线 + `rebuild_snapshot` 过滤（gateway，MockUpstream 集成测试）
3. config 两个新字段（config）
4. `GET /api/disabled` 开放端点（dashboard）
5. admin 鉴权中间件 + handlers + 挂载 + `AppState.admin_token`（dashboard，内聚一任务避免 dead_code）
6. 工具禁用 populated e2e（dashboard，MockUpstream）
7. `about.rs` `admin_enabled` 裸 bool（dashboard）
8. main.rs 装配（token fail-fast + DisableSet 注入）（mcpgw）
9. 前端：`api.js` + `admin.svelte.js` + About 写访问段（ui）
10. 前端：Upstreams/Tools 徽标+开关+Disabled tools 区 + 重建 dist（ui）
11. 文档 L1–L4 同步

## 门禁（每个 Rust 任务结束都跑）

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
UI 任务额外：`cd crates/dashboard/ui && npm run build` 且 `git diff --stat` 显示 dist 已更新、`cargo build` 仍 node-free 可复现。

---

### Task 1: `DisableSet` 核心 + 持久化（gateway crate）

**Files:**
- Create: `crates/gateway/src/disable.rs`
- Modify: `crates/gateway/Cargo.toml`（加 serde/serde_json 依赖）
- Modify: `crates/gateway/src/lib.rs`（`mod disable` + 重导出，紧跟现有 `use` 区之后）

纯内存 + 可选 JSON 状态文件。`BTreeSet` 天然有序；变更才落盘；落盘 best-effort 原子写（temp→fsync→rename），失败只 warn 不阻断、不回滚内存。

- [ ] **Step 1: 加依赖**

`crates/gateway/Cargo.toml` 的 `[dependencies]` 末尾加：

```toml
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: 写 `disable.rs`（实现 + 单元测试）**

创建 `crates/gateway/src/disable.rs`：

```rust
//! Runtime disable set: temporarily hide an upstream (namespace) or a single qualified tool from
//! the gateway's snapshot. Pure in-memory `BTreeSet`s with optional JSON persistence (atomic,
//! best-effort) so disables survive a restart when `[dashboard].disabled_state_path` is set.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// The disabled names, serialized form is `DisabledSnapshot`.
#[derive(Default)]
struct DisabledState {
    upstreams: BTreeSet<String>,
    tools: BTreeSet<String>,
}

/// Ordered, owned view of the disabled set — the `GET /api/disabled` body and the on-disk form.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DisabledSnapshot {
    #[serde(default)]
    pub upstreams: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// In-memory disable set with optional JSON persistence. Cheaply shared behind an `Arc`.
pub struct DisableSet {
    inner: RwLock<DisabledState>,
    path: Option<PathBuf>,
}

impl Default for DisableSet {
    /// Empty, no persistence (in-memory only) — the default a plain `GatewayState` carries.
    fn default() -> Self {
        Self {
            inner: RwLock::new(DisabledState::default()),
            path: None,
        }
    }
}

impl DisableSet {
    /// Build from an optional state-file path. With a path that exists, load it; a missing file is
    /// an empty set (normal); a corrupt/unreadable file degrades to empty + `warn!` (self-healing,
    /// never blocks startup). With no path, an empty in-memory set.
    pub fn load_or_new(path: Option<PathBuf>) -> Self {
        let mut state = DisabledState::default();
        if let Some(p) = path.as_deref() {
            match std::fs::read_to_string(p) {
                Ok(text) => match serde_json::from_str::<DisabledSnapshot>(&text) {
                    Ok(snap) => {
                        state.upstreams = snap.upstreams.into_iter().collect();
                        state.tools = snap.tools.into_iter().collect();
                    }
                    Err(e) => {
                        tracing::warn!(path = %p.display(), error = %e,
                            "disabled state file is corrupt; starting with an empty disable set");
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // first run: empty
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e,
                        "could not read disabled state file; starting with an empty disable set");
                }
            }
        }
        Self {
            inner: RwLock::new(state),
            path,
        }
    }

    pub fn is_upstream_disabled(&self, name: &str) -> bool {
        self.inner.read().unwrap().upstreams.contains(name)
    }

    pub fn is_tool_disabled(&self, qualified: &str) -> bool {
        self.inner.read().unwrap().tools.contains(qualified)
    }

    /// Ordered snapshot (the API body + the persisted form).
    pub fn snapshot(&self) -> DisabledSnapshot {
        let s = self.inner.read().unwrap();
        DisabledSnapshot {
            upstreams: s.upstreams.iter().cloned().collect(),
            tools: s.tools.iter().cloned().collect(),
        }
    }

    pub fn disable_upstream(&self, name: &str) -> bool {
        self.mutate(|s| s.upstreams.insert(name.to_string()))
    }
    pub fn enable_upstream(&self, name: &str) -> bool {
        self.mutate(|s| s.upstreams.remove(name))
    }
    pub fn disable_tool(&self, qualified: &str) -> bool {
        self.mutate(|s| s.tools.insert(qualified.to_string()))
    }
    pub fn enable_tool(&self, qualified: &str) -> bool {
        self.mutate(|s| s.tools.remove(qualified))
    }

    /// Apply `f` under the write lock; if it reports a change, persist (best-effort) before
    /// releasing the lock so the on-disk form matches the in-memory set. Returns whether changed.
    fn mutate(&self, f: impl FnOnce(&mut DisabledState) -> bool) -> bool {
        let mut s = self.inner.write().unwrap();
        let changed = f(&mut s);
        if changed {
            if let Some(p) = self.path.as_deref() {
                let snap = DisabledSnapshot {
                    upstreams: s.upstreams.iter().cloned().collect(),
                    tools: s.tools.iter().cloned().collect(),
                };
                persist(p, &snap);
            }
        }
        changed
    }
}

/// Best-effort atomic write: serialize to a sibling temp file, fsync, then rename over `path`.
/// Any failure is logged and swallowed — the in-memory set stays authoritative and the next
/// successful toggle rewrites the whole file. (A separate `write_atomic` fn avoids the
/// immediately-invoked-closure that would trip `clippy::redundant_closure_call`.)
fn persist(path: &Path, snap: &DisabledSnapshot) {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = write_atomic(&tmp, path, snap) {
        let _ = std::fs::remove_file(&tmp); // don't leak a temp on failure
        tracing::warn!(path = %path.display(), error = %e,
            "could not persist disabled state file (in-memory set is still authoritative)");
    }
}

fn write_atomic(tmp: &Path, path: &Path, snap: &DisabledSnapshot) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(snap)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut f = std::fs::File::create(tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mcpgw-dis-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn disable_enable_report_changed_and_are_idempotent() {
        let d = DisableSet::default();
        assert!(d.disable_upstream("a"));      // newly inserted -> changed
        assert!(!d.disable_upstream("a"));     // already there -> no change
        assert!(d.is_upstream_disabled("a"));
        assert!(d.enable_upstream("a"));       // removed -> changed
        assert!(!d.enable_upstream("a"));      // absent -> no change
        assert!(!d.is_upstream_disabled("a"));
    }

    #[test]
    fn tool_and_upstream_axes_are_independent() {
        let d = DisableSet::default();
        d.disable_tool("srv__echo");
        assert!(d.is_tool_disabled("srv__echo"));
        assert!(!d.is_upstream_disabled("srv__echo")); // tool name is not an upstream name
        assert!(!d.is_tool_disabled("srv__greet"));
    }

    #[test]
    fn snapshot_is_sorted() {
        let d = DisableSet::default();
        d.disable_upstream("b");
        d.disable_upstream("a");
        d.disable_tool("z__t");
        d.disable_tool("a__t");
        let s = d.snapshot();
        assert_eq!(s.upstreams, vec!["a", "b"]);
        assert_eq!(s.tools, vec!["a__t", "z__t"]);
    }

    #[test]
    fn persists_and_reloads_across_instances() {
        let p = tmp("roundtrip");
        let _ = std::fs::remove_file(&p);
        {
            let d = DisableSet::load_or_new(Some(p.clone()));
            d.disable_upstream("flaky");
            d.disable_tool("github__delete_repo");
        }
        // No leftover temp file beside the state file.
        assert!(!p.with_extension(format!("tmp.{}", std::process::id())).exists());
        let reloaded = DisableSet::load_or_new(Some(p.clone()));
        assert!(reloaded.is_upstream_disabled("flaky"));
        assert!(reloaded.is_tool_disabled("github__delete_repo"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_loads_empty() {
        let p = tmp("missing");
        let _ = std::fs::remove_file(&p);
        let d = DisableSet::load_or_new(Some(p));
        assert_eq!(d.snapshot(), DisabledSnapshot::default());
    }

    #[test]
    fn corrupt_file_loads_empty_without_panic() {
        let p = tmp("corrupt");
        std::fs::write(&p, b"{ this is not json").unwrap();
        let d = DisableSet::load_or_new(Some(p.clone()));
        assert_eq!(d.snapshot(), DisabledSnapshot::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unwritable_path_is_best_effort_and_keeps_memory_state() {
        // A path under a non-existent directory: persistence fails, but the in-memory set still
        // reflects the change and no panic occurs.
        let p = std::env::temp_dir()
            .join(format!("mcpgw-dis-nodir-{}", std::process::id()))
            .join("state.json");
        let d = DisableSet::load_or_new(Some(p));
        assert!(d.disable_upstream("x")); // still reports changed
        assert!(d.is_upstream_disabled("x")); // memory is authoritative despite write failure
    }
}
```

- [ ] **Step 3: 接进 crate（`lib.rs`）**

在 `crates/gateway/src/lib.rs` 顶部模块声明区加：

```rust
mod disable;
pub use disable::{DisableSet, DisabledSnapshot};
```

- [ ] **Step 4: 跑测试 + 门禁**

```bash
cargo test -p gateway disable:: -- --nocapture
cargo fmt --all --check && cargo clippy -p gateway --all-targets --all-features -- -D warnings
```
Expected: 9 个 disable 测试全过；fmt/clippy 净。

- [ ] **Step 5: Commit**

```bash
git add crates/gateway/src/disable.rs crates/gateway/src/lib.rs crates/gateway/Cargo.toml Cargo.lock
git commit -m "feat(gateway): DisableSet 内存禁用集 + 可选 JSON 持久化（原子写、best-effort）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: `GatewayState` 接线 + `rebuild_snapshot` 过滤（gateway crate）

**Files:**
- Modify: `crates/gateway/src/lib.rs`（GatewayState 字段 + builder/getter + rebuild 过滤；`build()` ≈ line 72-84；rebuild 循环 ≈ line 135-168）
- Create: `crates/gateway/tests/disable.rs`（MockUpstream 集成测试）

被禁用上游**连 ingest task 都不 spawn**（故 `summary.ingested` 不含它，可观测证明）；被禁用单工具在 upsert 时跳过。注入在首次 rebuild 前生效。

- [ ] **Step 1: 写集成测试 `crates/gateway/tests/disable.rs`**

```rust
use std::sync::Arc;

use gateway::{DisableSet, GatewayState};
use metatools::{get_tool_details, search_tools};
use rmcp::ServiceExt;
use upstream::connection::UpstreamHandle;
use upstream::testkit::MockUpstream;

async fn connect_mock(name: &str) -> (UpstreamHandle, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let join = tokio::spawn(async move {
        let svc = MockUpstream::new().serve(server_io).await.unwrap();
        svc.waiting().await.unwrap();
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    (handle, join)
}

#[tokio::test]
async fn disabled_upstream_is_skipped_at_ingest_and_hidden_then_restored() {
    let disabled = Arc::new(DisableSet::default());
    let state = GatewayState::new("bm25").unwrap().with_disabled(disabled.clone());
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));

    // Disable BEFORE rebuild: the upstream is not even ingested (absent from summary.ingested)
    // and its tools are unsearchable.
    disabled.disable_upstream("mock");
    let summary = state.rebuild_snapshot().await.unwrap();
    assert!(
        !summary.ingested.contains(&"mock".to_string()),
        "disabled upstream must not be ingested: {summary:?}"
    );
    assert!(search_tools(&state.snapshot(), "echo", 5).await.is_empty());

    // Re-enable + rebuild: tools come back (connection was preserved).
    disabled.enable_upstream("mock");
    let summary = state.rebuild_snapshot().await.unwrap();
    assert!(summary.ingested.contains(&"mock".to_string()));
    let hits = search_tools(&state.snapshot(), "echo", 5).await;
    assert!(hits.iter().any(|s| s.name == "mock__echo"), "hits: {hits:?}");

    join.abort();
}

#[tokio::test]
async fn disabled_single_tool_is_hidden_but_siblings_remain() {
    let disabled = Arc::new(DisableSet::default());
    let state = GatewayState::new("bm25").unwrap().with_disabled(disabled.clone());
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));

    disabled.disable_tool("mock__echo");
    state.rebuild_snapshot().await.unwrap();

    let snap = state.snapshot();
    assert!(get_tool_details(&snap, "mock__echo").is_none(), "echo must be hidden");
    assert!(get_tool_details(&snap, "mock__greet").is_some(), "greet must remain");
    assert!(
        !search_tools(&snap, "echo", 5).await.iter().any(|s| s.name == "mock__echo"),
        "disabled tool must not be searchable"
    );

    join.abort();
}
```

- [ ] **Step 2: 跑测试，确认编译失败**

Run: `cargo test -p gateway --test disable`
Expected: 编译错误 `no method named with_disabled` / `no method named disabled`。

- [ ] **Step 3: 实现 GatewayState 改动（`crates/gateway/src/lib.rs`）**

3a. 给 struct 加字段（`GatewayState { ... }` 内，`last_summary` 之后）：

```rust
    /// Runtime disable set (default empty). Read on every rebuild to skip disabled upstreams/tools.
    disabled: Arc<disable::DisableSet>,
```

3b. `build()` 初始化（`last_summary: Arc::new(ArcSwapOption::empty()),` 之后）：

```rust
            disabled: Arc::new(disable::DisableSet::default()),
```

3c. 在 `registry()` getter 之后加 builder + getter：

```rust
    /// Replace the disable set (assembly-time injection, before the first rebuild). Cheap-clone
    /// `Arc` shared with the dashboard via the same `GatewayState`.
    pub fn with_disabled(mut self, disabled: Arc<disable::DisableSet>) -> Self {
        self.disabled = disabled;
        self
    }

    /// The runtime disable set (read by rebuild; mutated by the dashboard admin API).
    pub fn disabled(&self) -> &disable::DisableSet {
        self.disabled.as_ref()
    }

    /// An owned `Arc` clone of the disable set — for callers that move it across an `.await`
    /// (e.g. the dashboard admin handler runs the synchronous, fsync-ing mutation in `spawn_blocking`).
    pub fn disabled_arc(&self) -> Arc<disable::DisableSet> {
        self.disabled.clone()
    }
```

3d. `rebuild_snapshot` ingest 循环：在 `for name in self.registry.server_names() {` 之后、`if let Some(handle)` 之前加跳过：

```rust
        for name in self.registry.server_names() {
            if self.disabled.is_upstream_disabled(&name) {
                continue; // disabled upstream: not even ingested
            }
            if let Some(handle) = self.registry.get(&name) {
```

3e. `rebuild_snapshot` upsert 循环：把 `for tool in local.iter() { catalog.upsert(tool.clone()); }` 改为：

```rust
                Ok(Ok(_dupes)) => {
                    for tool in local.iter() {
                        if self.disabled.is_tool_disabled(&tool.qualified_name()) {
                            continue; // disabled single tool: skip upsert
                        }
                        catalog.upsert(tool.clone());
                    }
                    summary.ingested.push(name);
                }
```

- [ ] **Step 4: 跑测试 + 门禁**

```bash
cargo test -p gateway --test disable
cargo test -p gateway   # 既有 rebuild/单元测试不回归
cargo fmt --all --check && cargo clippy -p gateway --all-targets --all-features -- -D warnings
```
Expected: 2 个 disable 集成测试过；既有测试不回归；fmt/clippy 净。

- [ ] **Step 5: Commit**

```bash
git add crates/gateway/src/lib.rs crates/gateway/tests/disable.rs
git commit -m "feat(gateway): rebuild_snapshot 读 DisableSet 跳过禁用上游 ingest 与禁用单工具

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: config 两个新字段（config crate）

**Files:**
- Modify: `crates/config/src/lib.rs`（`DashboardConfig` struct + `Default` impl + 测试）

二者皆 `Option<String>`、默认 `None` → 行为不变。`#[serde(default, deny_unknown_fields)]` 已在 struct 上，新字段无需逐项属性。

- [ ] **Step 1: 写测试**

在 config 测试模块加（紧邻既有 `omitting_dashboard_section_is_disabled` 等）：

```rust
    #[test]
    fn dashboard_admin_and_disabled_path_default_none() {
        let d = Config::from_toml_str("").unwrap().dashboard;
        assert!(d.admin_token_env.is_none());
        assert!(d.disabled_state_path.is_none());
    }

    #[test]
    fn dashboard_parses_admin_and_disabled_path() {
        let d = Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_DASH_ADMIN\"\ndisabled_state_path = \"mcpgw-disabled.json\"\n",
        )
        .unwrap()
        .dashboard;
        assert_eq!(d.admin_token_env.as_deref(), Some("MCPGW_DASH_ADMIN"));
        assert_eq!(d.disabled_state_path.as_deref(), Some("mcpgw-disabled.json"));
    }
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p config dashboard_parses_admin_and_disabled_path`
Expected: 编译错误 `no field admin_token_env`。

- [ ] **Step 3: 加字段**

`DashboardConfig` struct 在 `payload_max_bytes` 之后加：

```rust
    /// Env var name holding the dashboard admin Bearer token. None -> admin write API disabled.
    /// The secret is referenced by env name only (resolved fail-fast at `serve` start).
    pub admin_token_env: Option<String>,
    /// Path to persist the runtime disable set (JSON). None -> in-memory only (lost on restart).
    pub disabled_state_path: Option<String>,
```

`Default for DashboardConfig` 在 `payload_max_bytes: 16384,` 之后加：

```rust
            admin_token_env: None,
            disabled_state_path: None,
```

- [ ] **Step 4: 跑测试 + 门禁**

```bash
cargo test -p config
cargo fmt --all --check && cargo clippy -p config --all-targets --all-features -- -D warnings
```
Expected: 新 2 测试过；既有不回归。

- [ ] **Step 5: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): [dashboard] 加 admin_token_env / disabled_state_path（默认 None）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: `GET /api/disabled` 开放端点（dashboard crate）

**Files:**
- Modify: `crates/dashboard/src/api.rs`（纯函数 `disabled(&AppState)` + 测试）
- Modify: `crates/dashboard/src/lib.rs`（`h_disabled` 包装 + 路由）

沿用"每端点一个 `api::` 纯函数 + 薄 `h_*` 包装"的既有惯例。读 `state.gateway.disabled().snapshot()`，无鉴权、始终挂载。

- [ ] **Step 1: 写 `api::disabled` 测试（`crates/dashboard/src/api.rs` 测试模块）**

```rust
    #[tokio::test]
    async fn disabled_endpoint_reflects_gateway_disable_set() {
        let st = seeded_state().await;
        // Empty by default.
        assert_eq!(disabled(&st), gateway::DisabledSnapshot::default());
        // Mutating the shared gateway disable set is reflected through the endpoint.
        st.gateway.disabled().disable_upstream("github");
        st.gateway.disabled().disable_tool("github__create_issue");
        let snap = disabled(&st);
        assert_eq!(snap.upstreams, vec!["github"]);
        assert_eq!(snap.tools, vec!["github__create_issue"]);
    }
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dashboard disabled_endpoint_reflects_gateway_disable_set`
Expected: 编译错误 `cannot find function disabled`。

- [ ] **Step 3: 实现纯函数（`api.rs`，与其它 `pub fn overview/upstreams` 并列）**

```rust
/// `/api/disabled` body: the current runtime disable set (open/read-only).
pub fn disabled(s: &AppState) -> gateway::DisabledSnapshot {
    s.gateway.disabled().snapshot()
}
```

- [ ] **Step 4: 加 handler + 路由（`lib.rs`）**

在 `h_about` 旁加包装：

```rust
async fn h_disabled(State(s): State<Arc<AppState>>) -> Json<gateway::DisabledSnapshot> {
    Json(api::disabled(&s))
}
```

`build_dashboard_router` 路由链里（`/api/about` 之后、`.fallback` 之前）加：

```rust
        .route("/api/disabled", get(h_disabled))
```

- [ ] **Step 5: 跑测试 + 门禁**

```bash
cargo test -p dashboard
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings
```
Expected: 新测试过；既有不回归。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/src/api.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): GET /api/disabled 开放只读端点（反映运行时禁用集）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: admin 鉴权中间件 + disable/enable handlers + 挂载（dashboard crate）

**Files:**
- Modify: `crates/dashboard/Cargo.toml`（加 `subtle = "2"`）
- Modify: `crates/dashboard/src/api.rs`（`AppState` 加 `admin_token` 字段 + handler 测试 + seeded_state 更新）
- Create: `crates/dashboard/src/admin.rs`（`presented_bearer` + `authorize` + `require_admin_token` + 4 handlers + 纯测试）
- Modify: `crates/dashboard/src/lib.rs`（`mod admin` + admin 子路由 route_layer 挂载）
- Modify: `crates/mcpgw/src/main.rs`（AppState 字面量加 `admin_token: None`，Task 8 再填真值——保持本任务编译绿）

> **为何合一**：中间件/handlers/挂载互相依赖，分拆会在中间任务留下 `dead_code`（clippy `-D warnings` 是硬门）。整块落地无悬空代码；步骤仍 bite-sized。
>
> **有意省略 admin 串行 Mutex**：spec 提的"外包 tokio::Mutex"只是避免 rebuild 风暴的优化，非正确性所需。每个 handler 是"先改集(RwLock 原子)→再 await rebuild"，而 `rebuild_snapshot` 自带 `rebuild_lock` 串行、且**在 rebuild 时读当前禁用集**：最后获得 rebuild_lock 的那次 rebuild 必读到所有已完成的写 → 最终快照与持久化均收敛一致。故不引入额外 AppState 字段。

- [ ] **Step 1: 加 subtle 依赖**

`crates/dashboard/Cargo.toml` 的 `[dependencies]` 加：

```toml
subtle = "2"
```

- [ ] **Step 2: `AppState` 加字段 + 更新两个构造点**

`crates/dashboard/src/api.rs` 的 `AppState` 结构（`about` 字段之后）加：

```rust
    /// Admin Bearer token (env-resolved at startup). None -> /api/admin/* returns 404.
    pub admin_token: Option<std::sync::Arc<str>>,
```

`api.rs` 测试里的 `seeded_state()` 的 AppState 字面量加 `admin_token: None,`；
`crates/mcpgw/src/main.rs` 的 `dashboard::AppState { ... }` 字面量（约 line 424-451）也加 `admin_token: None,`（Task 8 改真值）。

- [ ] **Step 3: 写 `admin.rs`（实现 + 纯测试）**

创建 `crates/dashboard/src/admin.rs`：

```rust
//! Admin write subsystem: Bearer-gated runtime disable/enable handlers + the auth middleware.
//! Gated-mount semantics: no admin token configured -> the middleware returns 404 (admin
//! effectively absent, existence not leaked); wrong/absent Bearer -> 401; match -> pass-through.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use gateway::DisableSet;
use subtle::ConstantTimeEq;

use crate::api::AppState;

/// Parse `Authorization: Bearer <token>` (scheme case-insensitive; empty token = absent).
fn presented_bearer(req: &Request) -> Option<String> {
    let raw = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[derive(Debug, PartialEq)]
enum AdminAuth {
    NotConfigured,
    Denied,
    Allowed,
}

/// Pure auth decision. No token configured -> NotConfigured (-> 404, no leak); configured +
/// matching Bearer -> Allowed; otherwise Denied (-> 401). Constant-time token compare.
fn authorize(configured: Option<&str>, presented: Option<&str>) -> AdminAuth {
    match configured {
        None => AdminAuth::NotConfigured,
        Some(expected) => match presented {
            Some(tok) if expected.as_bytes().ct_eq(tok.as_bytes()).into() => AdminAuth::Allowed,
            _ => AdminAuth::Denied,
        },
    }
}

/// Middleware on `/api/admin/*`: maps the auth decision to 404 / 401 / pass-through.
pub async fn require_admin_token(State(s): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let presented = presented_bearer(&req);
    match authorize(s.admin_token.as_deref(), presented.as_deref()) {
        AdminAuth::NotConfigured => StatusCode::NOT_FOUND.into_response(),
        AdminAuth::Denied => StatusCode::UNAUTHORIZED.into_response(),
        AdminAuth::Allowed => next.run(req).await,
    }
}

/// Run a disable-set mutation OFF the async worker — `DisableSet` persists synchronously (an
/// `fsync` under a lock), so doing it inline would block an axum executor thread. Then rebuild the
/// snapshot and return the updated set. (`DisableSet: Send + Sync` behind the `Arc`.)
async fn mutate_and_rebuild(
    s: &Arc<AppState>,
    mutate: impl FnOnce(&DisableSet) -> bool + Send + 'static,
) -> Response {
    let d = s.gateway.disabled_arc();
    let _ = tokio::task::spawn_blocking(move || mutate(&d)).await;
    let _ = s.gateway.rebuild_snapshot().await;
    Json(s.gateway.disabled().snapshot()).into_response()
}

pub async fn disable_upstream(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    let dis = s.gateway.disabled();
    if dis.is_upstream_disabled(&name) {
        return Json(dis.snapshot()).into_response(); // idempotent no-op
    }
    if !s.upstreams.iter().any(|u| u.name == name) {
        return StatusCode::NOT_FOUND.into_response(); // unknown upstream
    }
    mutate_and_rebuild(&s, move |ds| ds.disable_upstream(&name)).await
}

pub async fn enable_upstream(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if !s.gateway.disabled().is_upstream_disabled(&name) {
        return Json(s.gateway.disabled().snapshot()).into_response();
    }
    mutate_and_rebuild(&s, move |ds| ds.enable_upstream(&name)).await
}

pub async fn disable_tool(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if s.gateway.disabled().is_tool_disabled(&name) {
        return Json(s.gateway.disabled().snapshot()).into_response();
    }
    if s.gateway.snapshot().catalog().get(&name).is_none() {
        return StatusCode::NOT_FOUND.into_response(); // tool not currently visible
    }
    mutate_and_rebuild(&s, move |ds| ds.disable_tool(&name)).await
}

pub async fn enable_tool(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if !s.gateway.disabled().is_tool_disabled(&name) {
        return Json(s.gateway.disabled().snapshot()).into_response();
    }
    mutate_and_rebuild(&s, move |ds| ds.enable_tool(&name)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn req(auth: Option<&str>) -> Request {
        let mut b = axum::http::Request::builder();
        if let Some(v) = auth {
            b = b.header(AUTHORIZATION, v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn presented_bearer_parses_scheme_case_insensitively_and_rejects_others() {
        for h in ["Bearer tok", "bearer tok", "BEARER tok"] {
            assert_eq!(presented_bearer(&req(Some(h))), Some("tok".to_string()));
        }
        assert_eq!(presented_bearer(&req(Some("Bearer "))), None); // empty token
        assert_eq!(presented_bearer(&req(Some("Basic tok"))), None);
        assert_eq!(presented_bearer(&req(None)), None);
    }

    #[test]
    fn authorize_maps_config_and_token_to_decision() {
        assert_eq!(authorize(None, Some("x")), AdminAuth::NotConfigured);
        assert_eq!(authorize(Some("sekret"), Some("sekret")), AdminAuth::Allowed);
        assert_eq!(authorize(Some("sekret"), Some("wrong")), AdminAuth::Denied);
        assert_eq!(authorize(Some("sekret"), None), AdminAuth::Denied);
    }
}
```

- [ ] **Step 4: handler 测试（`api.rs` 测试模块，复用 `seeded_state`）**

在 `api.rs` 的 `#[cfg(test)] mod tests` 内加（顶部确保 `use axum::extract::{Path, State};` 与 `use axum::http::StatusCode;`）：

```rust
    #[tokio::test]
    async fn admin_disable_enable_upstream_roundtrip() {
        let st = Arc::new(seeded_state().await); // upstreams = ["github"]
        let r = crate::admin::disable_upstream(State(st.clone()), Path("github".into())).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(disabled(&st).upstreams, vec!["github"]);
        let r = crate::admin::enable_upstream(State(st.clone()), Path("github".into())).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(disabled(&st).upstreams.is_empty());
    }

    #[tokio::test]
    async fn admin_disable_unknown_upstream_is_404() {
        let st = Arc::new(seeded_state().await);
        let r = crate::admin::disable_upstream(State(st.clone()), Path("nope".into())).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_disable_absent_tool_is_404() {
        let st = Arc::new(seeded_state().await); // empty catalog (no upstreams ingested)
        let r = crate::admin::disable_tool(State(st.clone()), Path("github__x".into())).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_disable_upstream_is_idempotent() {
        let st = Arc::new(seeded_state().await);
        let _ = crate::admin::disable_upstream(State(st.clone()), Path("github".into())).await;
        let r = crate::admin::disable_upstream(State(st.clone()), Path("github".into())).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(disabled(&st).upstreams, vec!["github"]); // still single entry
    }
```

- [ ] **Step 5: 挂载（`lib.rs`）**

`mod` 区加 `mod admin;`；确保 `use axum::routing::{get, post};`（加 `post`）。
`build_dashboard_router` 里在构造 `router` 之前建 admin 子路由，并 `.merge(admin)`（放在 `.route("/api/disabled", ...)` 之后、`.fallback(...)` 之前）：

```rust
    let admin = axum::Router::new()
        .route("/api/admin/upstreams/{name}/disable", post(admin::disable_upstream))
        .route("/api/admin/upstreams/{name}/enable", post(admin::enable_upstream))
        .route("/api/admin/tools/{name}/disable", post(admin::disable_tool))
        .route("/api/admin/tools/{name}/enable", post(admin::enable_tool))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            admin::require_admin_token,
        ));
```
然后在路由链加 `.merge(admin)`。

- [ ] **Step 6: 跑测试 + 门禁**

```bash
cargo test -p dashboard
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings
cargo build --locked
```
Expected: admin 纯测试 + handler 测试全过；既有不回归；main 仍编译（`admin_token: None`）。

- [ ] **Step 7: Commit**

```bash
git add crates/dashboard/Cargo.toml crates/dashboard/src/admin.rs crates/dashboard/src/api.rs crates/dashboard/src/lib.rs crates/mcpgw/src/main.rs Cargo.lock
git commit -m "feat(dashboard): admin 写子系统——Bearer 中间件 + disable/enable handlers + 挂载

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: 工具禁用 populated e2e（dashboard crate，MockUpstream）

**Files:**
- Modify: `crates/dashboard/Cargo.toml`（`[dev-dependencies]` 加 `upstream`(testkit) + `rmcp`）
- Modify: `crates/dashboard/src/api.rs`（测试模块：抽出 `make_state(gw)` + `gateway_with_mock` + 成功路径测试）

`seeded_state` 的 gateway 无上游、catalog 为空，故 Task 5 只能测"工具不存在→404"。本任务接一个 MockUpstream 跑一次真实 rebuild，覆盖**工具存在→禁用成功→catalog 消失→/api/disabled 反映→启用恢复**。

- [ ] **Step 1: 加 dev-deps**

`crates/dashboard/Cargo.toml` 的 `[dev-dependencies]` 加：

```toml
upstream = { path = "../upstream", features = ["testkit"] }
rmcp = { workspace = true, features = ["client", "server", "macros", "transport-io"] }
```

- [ ] **Step 2: 重构 `seeded_state` → `make_state(gw)` + 加 mock 助手与测试**

把 `api.rs` 测试模块里现有的 `async fn seeded_state() -> AppState { ... }` 替换为：

```rust
    fn make_state(gw: Arc<GatewayState>) -> AppState {
        AppState {
            gateway: gw,
            metrics: Arc::new(MetricsSink::new()),
            discovery: None,
            calls: None,
            upstreams: vec![UpstreamInfo {
                name: "github".into(),
                transport: "stdio".into(),
            }],
            strategy: "bm25".into(),
            audit_path: None,
            discovery_path: None,
            started_at: Instant::now(),
            about: crate::about::AboutInfo::from_config(
                &config::Config::default_from_empty(),
                crate::about::VersionInfo {
                    version: "0.0.0-test".into(),
                    git_sha: "test".into(),
                    build_time: "0".into(),
                },
            ),
            admin_token: None,
        }
    }

    async fn seeded_state() -> AppState {
        make_state(Arc::new(GatewayState::new("bm25").unwrap()))
    }

    /// A gateway with the testkit MockUpstream connected and rebuilt (catalog populated with
    /// `mock__echo`/`mock__greet`/...). Returns the server task to abort at test end.
    async fn gateway_with_mock(name: &str) -> (Arc<GatewayState>, tokio::task::JoinHandle<()>) {
        use rmcp::ServiceExt;
        let (server_io, client_io) = tokio::io::duplex(4096);
        let join = tokio::spawn(async move {
            let svc = upstream::testkit::MockUpstream::new()
                .serve(server_io)
                .await
                .unwrap();
            svc.waiting().await.unwrap();
        });
        let handle = upstream::connection::UpstreamHandle::connect(name, client_io)
            .await
            .unwrap();
        let gw = Arc::new(GatewayState::new("bm25").unwrap());
        gw.registry().insert(Arc::new(handle));
        gw.rebuild_snapshot().await.unwrap();
        (gw, join)
    }
```

加测试：

```rust
    #[tokio::test]
    async fn admin_disable_tool_success_hides_it_and_reflects_in_disabled() {
        let (gw, join) = gateway_with_mock("mock").await;
        let st = Arc::new(make_state(gw));
        assert!(st.gateway.snapshot().catalog().get("mock__echo").is_some());

        let r = crate::admin::disable_tool(State(st.clone()), Path("mock__echo".into())).await;
        assert_eq!(r.status(), StatusCode::OK);
        // After the handler's rebuild: gone from the catalog, listed in /api/disabled.
        assert!(st.gateway.snapshot().catalog().get("mock__echo").is_none());
        assert_eq!(disabled(&st).tools, vec!["mock__echo"]);
        // Sibling tool is unaffected.
        assert!(st.gateway.snapshot().catalog().get("mock__greet").is_some());

        let r = crate::admin::enable_tool(State(st.clone()), Path("mock__echo".into())).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(st.gateway.snapshot().catalog().get("mock__echo").is_some());

        join.abort();
    }
```

- [ ] **Step 3: 跑测试 + 门禁**

```bash
cargo test -p dashboard admin_disable_tool_success_hides_it_and_reflects_in_disabled
cargo test -p dashboard
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings
```
Expected: 新 e2e 过；既有不回归。

- [ ] **Step 4: Commit**

```bash
git add crates/dashboard/Cargo.toml crates/dashboard/src/api.rs Cargo.lock
git commit -m "test(dashboard): MockUpstream 工具禁用 e2e（存在→禁用→catalog 消失→恢复）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 7: `about.rs` 加 `admin_enabled` 裸 bool（dashboard crate）

**Files:**
- Modify: `crates/dashboard/src/about.rs`（`DashboardInfo` + `from_config` + 测试）

镜像现有 `http_auth` 隐私模式：`admin_enabled` = 是否配了 `admin_token_env`（裸 bool，**绝不含 env 名/值**）。供前端 Settings 决定是否显示 token 输入框。

> **⚠️ 必处理的冲突**：现有测试 `http_auth_true_and_no_secrets_leak` 断言 JSON 不含子串 `"admin"`（因其 api_key `name = "admin"`）。新字段名 `admin_enabled` 会让 JSON 含 `"admin_enabled"` → 该断言误伤。**本任务须把那个测试里的哨兵标签从 `"admin"` 换成唯一标签**（见 Step 3）。

- [ ] **Step 1: 加字段**

`DashboardInfo`（`trace_path` 之后）加：

```rust
    /// True iff `[dashboard].admin_token_env` is configured (admin write API enabled). Never the
    /// env name or token value — a bare existence bool, mirroring `ServerInfo.http_auth`.
    pub admin_enabled: bool,
```

`from_config` 的 `DashboardInfo { ... }` 字面量（`trace_path: ...` 之后）加：

```rust
                admin_enabled: cfg.dashboard.admin_token_env.is_some(),
```

- [ ] **Step 2: 在首个映射测试里断言默认 false**

`from_config_maps_non_sensitive_fields` 末尾（`assert_eq!(a.version.version, "0.1.0");` 前后）加：

```rust
        assert!(!a.dashboard.admin_enabled, "no admin_token_env -> false");
```

- [ ] **Step 3: 改隐私测试哨兵 + 加 admin_enabled 无泄露测试**

把 `http_auth_true_and_no_secrets_leak` 里的 api_key 名从 `"admin"` 改为唯一标签，并同步改禁串列表：

```rust
        // ...api_key 段改为：
        //   [[server.http.api_key]]\nname = \"keylabel\"\nenv = \"SECRET_KEY\"\n
        // 禁串列表把 "admin" 改成 "keylabel"：
        for secret in [
            "SECRET_KEY",
            "REMOTE_TOKEN",
            "keylabel",
            "example.com",
            "bearer_env",
            "api_key",
        ] {
```

新增测试：

```rust
    #[test]
    fn admin_enabled_reflects_config_without_leaking_env_name() {
        let toml = "[retrieval]\nstrategy = \"bm25\"\n\
                    [dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_DASH_ADMIN\"\n";
        let cfg = config::Config::from_toml_str(toml).unwrap();
        let a = AboutInfo::from_config(&cfg, ver());
        assert!(a.dashboard.admin_enabled, "admin_token_env set -> true");
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            !json.contains("MCPGW_DASH_ADMIN"),
            "About JSON must not leak the admin env var name: {json}"
        );
    }
```

- [ ] **Step 4: 跑测试 + 门禁**

```bash
cargo test -p dashboard about::
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings
```
Expected: about 测试（含新 + 改后的隐私）全过。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/src/about.rs
git commit -m "feat(dashboard): About 加 admin_enabled 裸 bool（镜像 http_auth，绝不泄露 env 名）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 8: main.rs 装配（mcpgw bin）

**Files:**
- Modify: `crates/mcpgw/src/main.rs`（`resolve_admin_token` + DisableSet 加载注入 + AppState 填真值 + 测试）

`resolve_admin_token` 复用 `resolve_api_keys` 的 fail-fast 风格；DisableSet 在**首次 rebuild 前**经 `with_disabled` 注入（故启动即按持久化集过滤），与 `dashboard.enabled` 无关。

- [ ] **Step 1: 写测试（main.rs 测试模块，邻 `resolve_api_keys_*`）**

```rust
    #[test]
    fn resolve_admin_token_none_when_unconfigured() {
        let cfg = config::Config::from_toml_str("").unwrap();
        assert!(resolve_admin_token(&cfg).unwrap().is_none());
    }

    #[test]
    fn resolve_admin_token_reads_env_and_fails_fast() {
        std::env::set_var("MCPGW_T8_ADMIN", "s3cr3t");
        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_ADMIN\"\n",
        )
        .unwrap();
        assert_eq!(resolve_admin_token(&cfg).unwrap().as_deref(), Some("s3cr3t"));

        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_MISSING\"\n",
        )
        .unwrap();
        assert!(resolve_admin_token(&cfg).is_err());

        std::env::set_var("MCPGW_T8_EMPTY", "");
        let cfg = config::Config::from_toml_str(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_T8_EMPTY\"\n",
        )
        .unwrap();
        assert!(resolve_admin_token(&cfg).is_err(), "empty env -> fail-fast");
    }
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p mcpgw resolve_admin_token_reads_env_and_fails_fast`
Expected: 编译错误 `cannot find function resolve_admin_token`。

- [ ] **Step 3: 实现 `resolve_admin_token`（紧邻 `resolve_api_keys`，约 line 118 后）**

```rust
/// Resolve the optional dashboard admin Bearer token from `[dashboard].admin_token_env`. Returns
/// `Ok(None)` when unconfigured; fails fast if the referenced env var is missing or empty (so a
/// misconfigured admin token surfaces at startup rather than silently disabling writes).
fn resolve_admin_token(cfg: &config::Config) -> Result<Option<Arc<str>>, String> {
    let Some(env_name) = cfg.dashboard.admin_token_env.as_deref() else {
        return Ok(None);
    };
    let token = std::env::var(env_name)
        .map_err(|_| format!("[dashboard].admin_token_env: env {env_name:?} is not set"))?;
    if token.is_empty() {
        return Err(format!(
            "[dashboard].admin_token_env: env {env_name:?} is set but empty"
        ));
    }
    Ok(Some(Arc::from(token)))
}
```

- [ ] **Step 4: 注入 DisableSet（gateway 构建处，约 line 252-255）**

把：

```rust
    let state = Arc::new(
        gateway::GatewayState::with_backends(&cfg.retrieval.strategy, backends)
            .map_err(|e| e.to_string())?,
    );
```

改为：

```rust
    let disabled = Arc::new(gateway::DisableSet::load_or_new(
        cfg.dashboard
            .disabled_state_path
            .as_ref()
            .map(std::path::PathBuf::from),
    ));
    let state = Arc::new(
        gateway::GatewayState::with_backends(&cfg.retrieval.strategy, backends)
            .map_err(|e| e.to_string())?
            .with_disabled(disabled),
    );
```

- [ ] **Step 5: 解析 admin token + 填 AppState**

在 `let api_keys = resolve_api_keys(&cfg)?;`（约 line 284）之后加：

```rust
    let admin_token = resolve_admin_token(&cfg)?;
```

把 AppState 字面量里 Task 5 临时写的 `admin_token: None,`（约 line 424-451 块内）改为：

```rust
            admin_token: admin_token.clone(),
```

- [ ] **Step 6: 跑测试 + 门禁**

```bash
cargo test -p mcpgw
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked
```
Expected: resolve_admin_token 2 测试过；既有不回归；全工作区 clippy 净、build 绿。

- [ ] **Step 7: Commit**

```bash
git add crates/mcpgw/src/main.rs
git commit -m "feat(mcpgw): serve 解析 admin token(fail-fast) + 加载注入 DisableSet(首次 rebuild 前)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 9: 前端——admin 内存 token 态 + About 写访问段（dashboard UI）

**Files:**
- Create: `crates/dashboard/ui/src/lib/admin.svelte.js`
- Modify: `crates/dashboard/ui/src/lib/api.js`（加 `postJSON`）
- Modify: `crates/dashboard/ui/src/lib/About.svelte`（写访问段 + token 输入）

> 前端不引入测试框架（仓库惯例）：本任务"验证" = `npm run build` 通过 + `dist/` 再生 + `cargo build` 仍 node-free 内嵌。

- [ ] **Step 1: 创建 `admin.svelte.js`**

```js
// Admin write access. The token lives ONLY in memory (cleared on refresh) — never localStorage.
import { postJSON } from "./api.js";

export const admin = $state({ token: "" });

/** POST to an admin endpoint with the in-memory Bearer token. Returns the Response. */
export function adminPost(path) {
  return postJSON(path, admin.token);
}
```

- [ ] **Step 2: `api.js` 加 `postJSON`**

```js
/** POST with an optional Bearer token; returns the raw Response (caller inspects status). */
export async function postJSON(path, token) {
  return fetch(path, {
    method: "POST",
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
}
```

- [ ] **Step 3: About.svelte 加写访问段**

`<script>` 顶部 import：

```js
  import { admin } from "./admin.svelte.js";
```

在 `<h3>Server</h3>` 那段之后、`<h3>Upstreams ...</h3>` 之前插入：

```svelte
  <h3>Admin (write access)</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>status</th><td><span class="badge {info.dashboard.admin_enabled ? 'ok' : 'unknown'}">{info.dashboard.admin_enabled ? "enabled" : "disabled"}</span></td></tr>
  </tbody></table></div>
  {#if info.dashboard.admin_enabled}
    <p class="hint">Paste the admin token to unlock disable/enable controls. Held in memory only (cleared on refresh).</p>
    <input class="search" type="password" placeholder="admin token…" autocomplete="off" bind:value={admin.token} />
  {/if}
```

- [ ] **Step 4: 构建并验证 dist 再生**

```bash
cd crates/dashboard/ui && npm run build && cd -
git status --short crates/dashboard/ui/dist   # 应显示 dist/ 有改动
cargo build -p dashboard --locked              # 仍 node-free 内嵌新 dist
```
Expected: 构建成功；`dist/assets/*` 有新 hash 文件；cargo build 绿。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/ui/src/lib/admin.svelte.js crates/dashboard/ui/src/lib/api.js crates/dashboard/ui/src/lib/About.svelte crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): admin 内存 token 态 + About 写访问段（token 仅内存）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 10: 前端——禁用开关 + 徽标 + Disabled tools 区（dashboard UI）

**Files:**
- Create: `crates/dashboard/ui/src/lib/DisableToggle.svelte`
- Modify: `Upstreams.svelte` / `UpstreamDetail.svelte` / `Tools.svelte` / `ToolDetail.svelte`
- Modify: `crates/dashboard/ui/src/app.css`（加 `.admbtn` 样式）
- 重建 `crates/dashboard/ui/dist/`

隐藏式语义下被禁用工具不在 `/api/tools`，故 Tools 页用独立「Disabled tools」区（数据来自 `/api/disabled.tools`）承载 enable。开关仅在 `admin.token` 非空时显示（`DisableToggle` 内部判定）。`.admbtn` 是全局唯一类名。

- [ ] **Step 1: 创建 `DisableToggle.svelte`**

```svelte
<script>
  import { admin, adminPost } from "./admin.svelte.js";
  import { refreshNow } from "./refresh.svelte.js";
  // kind: "upstreams" | "tools"; disabled: current state (true => button enables, false => disables)
  let { kind, name, disabled } = $props();
  let busy = $state(false);
  async function toggle(e) {
    e.stopPropagation(); // don't trigger the row's navigation click
    busy = true;
    const action = disabled ? "enable" : "disable";
    try {
      await adminPost(`/api/admin/${kind}/${encodeURIComponent(name)}/${action}`);
      refreshNow();
    } finally {
      busy = false;
    }
  }
</script>

{#if admin.token}
  <button class="admbtn" onclick={toggle} disabled={busy} title={disabled ? "enable" : "disable"}>
    {disabled ? "enable" : "disable"}
  </button>
{/if}
```

- [ ] **Step 2: `Upstreams.svelte` —— 取 /api/disabled + 状态格加徽标/开关**

`<script>` import 加：

```js
  import DisableToggle from "./DisableToggle.svelte";
```

`load()` 改为同时取禁用集（在 `ups = await getJSON(...)` 后）：

```js
      dis = await getJSON("/api/disabled");
```

`<script>` 顶部状态加 `let dis = $state({ upstreams: [], tools: [] });` 与派生集：

```js
  const disUp = $derived(new Set(dis.upstreams));
```

把状态格（`<td><span class="badge {u.status}">{u.status}</span>...`）改为追加徽标 + 开关：

```svelte
            <td>
              <span class="badge {u.status}">{u.status}</span>
              {#if disUp.has(u.name)}<span class="badge skipped">disabled</span>{/if}
              {#if u.reason} <span class="muted">{u.reason}</span>{/if}
              <DisableToggle kind="upstreams" name={u.name} disabled={disUp.has(u.name)} />
            </td>
```

- [ ] **Step 3: `UpstreamDetail.svelte` —— 标题旁单上游开关 + 徽标**

import 加 `import DisableToggle from "./DisableToggle.svelte";`；`load()` 里在 `d` 赋值后加取禁用集：

```js
      dis = await getJSON("/api/disabled");
```

状态加 `let dis = $state({ upstreams: [], tools: [] });`（注意 import `getJSON`：`import { getJSON } from "./api.js";`，该组件原用 `fetch`——补这一行 import）。把标题改为：

```svelte
  <h2>
    {d.name}
    {#if dis.upstreams.includes(d.name)}<span class="badge skipped">disabled</span>{/if}
    <DisableToggle kind="upstreams" name={d.name} disabled={dis.upstreams.includes(d.name)} />
  </h2>
```

- [ ] **Step 4: `Tools.svelte` —— 行内 disable 开关 + 独立 Disabled tools 区**

import 加 `import DisableToggle from "./DisableToggle.svelte";`；`load()` 里在 `tools = t` 后加：

```js
      dis = await getJSON("/api/disabled");
```

状态加 `let dis = $state({ upstreams: [], tools: [] });`。把 name 格改为带 disable 开关（列表里的工具都是可见=未禁用，故 `disabled={false}`）：

```svelte
            <td class="mono">
              <a class="rl" href={href}>{t.name}</a>
              <DisableToggle kind="tools" name={t.name} disabled={false} />
            </td>
```

在主 `{#if tools}...{/if}` 之后追加 Disabled tools 区（无 token 也显示列表、仅 enable 按钮受 token 门控）：

```svelte
{#if dis.tools.length > 0}
  <h3>Disabled tools</h3>
  <div class="table-wrap"><div class="table-scroll"><table>
    <thead><tr><th>name</th><th></th></tr></thead>
    <tbody>
      {#each dis.tools as name}
        <tr>
          <td class="mono">{name} <span class="badge skipped">disabled</span></td>
          <td><DisableToggle kind="tools" {name} disabled={true} /></td>
        </tr>
      {/each}
    </tbody>
  </table></div></div>
{/if}
```

- [ ] **Step 5: `ToolDetail.svelte` —— 标题旁 disable 开关**

import 加 `import DisableToggle from "./DisableToggle.svelte";`。把 `<h2 class="mono">{d.name}</h2>` 改为：

```svelte
  <h2 class="mono">
    {d.name}
    <DisableToggle kind="tools" name={d.name} disabled={false} />
  </h2>
```
（注：禁用后该工具从 catalog 消失，`/api/tools/{name}` 转 404 → 详情页显示 "Tool not found"；再启用走 Tools 页的 Disabled tools 区——隐藏式语义的预期行为。）

- [ ] **Step 6: `app.css` 加 `.admbtn` 样式**

在文件末尾追加（贴合既有变量/风格）：

```css
.admbtn {
  margin-left: 8px;
  padding: 1px 8px;
  font-size: 12px;
  color: var(--muted);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
}
.admbtn:hover { color: var(--text); border-color: var(--muted); }
.admbtn:disabled { opacity: 0.5; cursor: default; }
```

- [ ] **Step 7: 构建并验证**

```bash
cd crates/dashboard/ui && npm run build && cd -
git status --short crates/dashboard/ui/dist
cargo build -p dashboard --locked
```
Expected: 构建成功；dist 再生；cargo build 绿。

- [ ] **Step 8: Commit**

```bash
git add crates/dashboard/ui/src/lib/DisableToggle.svelte crates/dashboard/ui/src/lib/Upstreams.svelte crates/dashboard/ui/src/lib/UpstreamDetail.svelte crates/dashboard/ui/src/lib/Tools.svelte crates/dashboard/ui/src/lib/ToolDetail.svelte crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): 禁用/启用开关 + disabled 徽标 + Disabled tools 区

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 11: 文档 L1–L4 同步 + .gitignore

**Files:**
- Modify: `.gitignore`
- Modify: `docs/L1-overview.md`、`docs/L2-components/{dashboard,gateway,config}.md`、`docs/L3-details/{dashboard,gateway,config}.md`、`docs/L4-api/{dashboard,config-lib,mcpgw-main}.md`

文档随代码同提交是仓库 DoD。本任务把"运行时禁用写子系统 B"贯穿四层；重点：把 dashboard 的"纯只读"不变量修订为"**默认只读；配置后可运行时禁用**"。

- [ ] **Step 1: `.gitignore` 加运行时状态文件**

追加一行：

```
mcpgw-disabled.json
```

- [ ] **Step 2: L1-overview.md**

- dashboard 段标题/描述："只读可视化面板（…已完成）" → 补一句"**新增可选写子系统 B（运行时临时禁用上游/工具，Bearer 鉴权；`GET /api/disabled` 开放）；默认不配置时仍纯只读**"。
- 端点列表 13 → **18**（补 `disabled` + 4 个 `admin/*`）。
- gateway/可观测段：补"`GatewayState` 持 `DisableSet`，`rebuild_snapshot` 跳过被禁用上游 ingest 与被禁用单工具"。

- [ ] **Step 3: L2-components/dashboard.md**

- 首段"只读、不改动任何网关状态"修订为"**默认只读**；配置 `[dashboard].admin_token_env` 后提供**运行时禁用写子系统**（仅 disable/enable，经 Bearer 鉴权），仍**不**做改配/重启/撤 key"。
- "公开接口"加一节：`admin.rs`（`require_admin_token` 中间件 + 4 handlers）；`api::disabled`。
- 端点段：13 → 18，列出 `GET /api/disabled`（开放）+ `POST /api/admin/{upstreams,tools}/{name}/{disable,enable}`（Bearer，未配 token → 404）。
- "不负责"段：把"任何写操作"修订为"除运行时禁用外的写操作（改配/重启/撤 key 仍不做）"。
- About：`DashboardInfo.admin_enabled` 裸 bool（镜像 `http_auth`，不含 env 名/值）。

- [ ] **Step 4: L2-components/gateway.md + L3-details/gateway.md**

- L2：`GatewayState` 公开接口表加 `with_disabled(Arc<DisableSet>)` / `disabled() -> &DisableSet`；`DisableSet`/`DisabledSnapshot` 类型；说明 rebuild 读 disabled。
- L3：`rebuild_snapshot` 算法补两处过滤（被禁用上游不 spawn ingest task → 不在 `summary.ingested`；被禁用单工具 upsert 时跳过）；`DisableSet` 持久化（可选 JSON、原子 temp→fsync→rename、best-effort、坏文件自愈）。

- [ ] **Step 5: L3-details/dashboard.md**

补"运行时禁用写子系统"小节：鉴权（env 引用、常量时间、未配 404/错 401）；读开放写鉴权；handler 流程（幂等优先→存在性校验→改集→持久化→`await rebuild`→回整集）；**in-flight 调用一次漏过**的并发说明；持久化与 `dashboard.enabled` 解耦。

- [ ] **Step 6: L2/L3 config + L4-api/config-lib.md**

- 三处 `DashboardConfig` 说明加 `admin_token_env`（env 引用、fail-fast）/ `disabled_state_path`（无自动默认、None=纯内存）两字段。

- [ ] **Step 7: L4-api/dashboard.md + L4-api/mcpgw-main.md**

- L4 dashboard：逐符号加 `admin::{require_admin_token, disable_upstream, enable_upstream, disable_tool, enable_tool}`、`api::disabled`、`AppState.admin_token`；端点表 +5。
- L4 mcpgw-main：`resolve_admin_token`（fail-fast）+ `DisableSet::load_or_new` 注入（首次 rebuild 前）+ AppState 接线。

- [ ] **Step 8: 一致性校验 + Commit**

```bash
grep -rn "18 个\|13 个\|/api/disabled\|/api/admin" docs/L1-overview.md docs/L2-components/dashboard.md docs/L4-api/dashboard.md
cargo build --locked   # 文档不影响构建，确认仓库整体仍绿
```
Expected: 端点计数处处一致为 18；新端点在 L1/L2/L4 均出现。

```bash
git add .gitignore docs/
git commit -m "docs: L1–L4 同步运行时禁用写子系统 B（端点 13→18、DisableSet、admin 鉴权、新配置字段）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 自检清单（实现完成后）

- [ ] 全门禁绿：`cargo fmt --all --check` / `cargo clippy --all-targets --all-features -D warnings` / `cargo test --all-features` / `cargo build --locked`。
- [ ] `npm run build` 可复现、`dist/` 已入库、`cargo build` 仍 node-free。
- [ ] 端点计数 18 在 L1/L2/L4 一致。
- [ ] 默认不配置时行为零变化（admin 全 404、禁用集空、纯只读）。
- [ ] 密钥/ token/env 名绝不入 `/api/about`、日志、状态文件。
