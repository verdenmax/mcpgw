# Dashboard 在线改配 + 热重载 实施计划（M5 写子系统 C）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让运维在 dashboard 里编辑整份 `mcpgw.toml`，严格校验后原子落盘(+.bak)、并对 `[[upstream]]` 增/删/改热重载（其余字段需重启），全程复用 M4 的 Bearer 鉴权。

**Architecture:** 全文编辑（无 toml_edit，验证后逐字写回）。校验逻辑（结构 + 全 env 引用可解析）以 `Arc<dyn Fn(&str)->Result<Config,String>>` 从 `main.rs` 注入 `AppState`（逻辑留 main、零耦合）。上游热重载是 `gateway` 的 `reconcile_upstreams`（三向 diff + 复用 `connect_all`/`registry.remove` + `rebuild_snapshot`，best-effort）。`GET/PUT /api/admin/config` 挂在 M4 的 Bearer 鉴权 admin 子路由（端点 18→20）。

**Tech Stack:** Rust（axum 0.8 / tokio / serde / arc-swap）+ Svelte 5 + Vite（dist 入库经 rust-embed 内嵌）。

**Spec:** `docs/superpowers/specs/2026-06-23-mcpgw-dashboard-config-edit-hot-reload-design.md`

---

## 文件结构（创建/修改一览）

**`gateway` crate**
- 修改 `crates/gateway/src/lib.rs` — 加 `ReconcileSummary` + `GatewayState::reconcile_upstreams(old, new, trigger)`（复用 `upstream::connect::connect_all` + `registry.remove` + `rebuild_snapshot`）。
- 创建 `crates/gateway/tests/reconcile.rs` — MockUpstream 集成测试（add/remove/change/unchanged/fail + 与 M4 禁用组合）。

**`dashboard` crate**
- 修改 `crates/dashboard/src/api.rs` — `AppState` 加 6 个字段（config_path/config_validator/config_write_lock/boot_config/applied_upstreams/rebuild_trigger）；更新 `seeded_state`/`make_state`。
- 创建 `crates/dashboard/src/admin_config.rs` — `get_config` + `put_config` handler + 原子写 + `needs_restart` diff + `ApplyResult`。
- 修改 `crates/dashboard/src/lib.rs` — `mod admin_config`；admin 子路由加 `GET/PUT /api/admin/config`。

**`mcpgw` bin**
- 修改 `crates/mcpgw/src/main.rs` — `run_serve` 加 `config_path` 参数（传 `cli.config`）；`prepare_state` 返回 `rebuild_trigger`（clone）；构造 `config_validator` 闭包；AppState 6 字段接线。

**UI（Svelte 5）**
- 修改 `crates/dashboard/ui/src/lib/admin.svelte.js` — 加 `adminGet` / `adminPut`。
- 创建 `crates/dashboard/ui/src/lib/Config.svelte` — TOML 编辑器 + Save + 结果/需重启展示。
- 修改 `crates/dashboard/ui/src/lib/Nav.svelte` — token 在时显示 Config 导航项；`App.svelte` 加路由。
- 重建 `crates/dashboard/ui/dist/*`。

**文档**：L1-overview、L2/L3 dashboard、L2/L3 gateway、L4 dashboard、L4 mcpgw-main 同步。
**`.gitignore`**：加 `*.bak`。

## 任务总览（6 个，逐个 spec+质量双重审查）

1. gateway `reconcile_upstreams` + 集成测试（gateway）
2. dashboard `AppState` 6 新字段 + main.rs 接线（dashboard + mcpgw）
3. `GET /api/admin/config`（dashboard）
4. `PUT /api/admin/config` + 原子写 + needs_restart + 边界测试扩展（dashboard）
5. 前端：adminGet/adminPut + Config.svelte + 导航 + 重建 dist（ui）
6. 文档 L1–L4 + .gitignore（docs）

> 备注：所有 `Config` 段已派生 `PartialEq`（无需额外任务）；`AppState` 派生 `Clone`、6 个新字段均 `Clone`。

## 门禁（每个 Rust 任务结束都跑）

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
UI 任务额外：`cd crates/dashboard/ui && npm run build` 且 `git status --short crates/dashboard/ui/dist` 显示 dist 已更新、`cargo build -p dashboard --locked` 仍 node-free。

---

### Task 1: gateway `reconcile_upstreams` + 三向 diff（gateway crate）

**Files:**
- Modify: `crates/gateway/Cargo.toml`（加 `config` 依赖）
- Modify: `crates/gateway/src/lib.rs`（`ReconcileSummary` + `ReconcilePlan` + `plan_upstream_reconcile` 纯函数 + `GatewayState::reconcile_upstreams` + 纯单元测试）
- Create: `crates/gateway/tests/reconcile.rs`（MockUpstream/坏命令 集成测试）

把"三向 diff"抽成纯函数（重点测），apply 复用既有 `connect_all`/`registry.remove`/`rebuild_snapshot`（best-effort）。

- [ ] **Step 1: 加 config 依赖**

`crates/gateway/Cargo.toml` 的 `[dependencies]` 加：

```toml
config = { path = "../config" }
```

- [ ] **Step 2: 写 `lib.rs` 的类型 + 纯函数 + 方法（实现 + 纯单元测试）**

在 `crates/gateway/src/lib.rs` 顶部 `use` 区后加（紧邻现有类型）：

```rust
/// One upstream-reconcile outcome (serialized into the dashboard's `ApplyResult`).
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReconcileSummary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub reconnected: Vec<String>,
    pub connect_failures: Vec<(String, String)>, // (name, error)
}

/// Pure three-way diff of upstream configs by name: which to drop, which to (re)connect.
struct ReconcilePlan {
    removed: Vec<String>,
    to_connect: Vec<config::UpstreamConfig>,
    added: Vec<String>,
    reconnected: Vec<String>,
}

fn plan_upstream_reconcile(
    old: &[config::UpstreamConfig],
    new: &[config::UpstreamConfig],
) -> ReconcilePlan {
    use std::collections::HashMap;
    let old_by: HashMap<&str, &config::UpstreamConfig> =
        old.iter().map(|u| (u.name.as_str(), u)).collect();
    let new_names: std::collections::HashSet<&str> =
        new.iter().map(|u| u.name.as_str()).collect();

    let mut plan = ReconcilePlan {
        removed: vec![],
        to_connect: vec![],
        added: vec![],
        reconnected: vec![],
    };
    for u in old {
        if !new_names.contains(u.name.as_str()) {
            plan.removed.push(u.name.clone());
        }
    }
    for u in new {
        match old_by.get(u.name.as_str()) {
            None => {
                plan.added.push(u.name.clone());
                plan.to_connect.push(u.clone());
            }
            Some(prev) if *prev != u => {
                plan.reconnected.push(u.name.clone());
                plan.to_connect.push(u.clone());
            }
            Some(_) => {} // unchanged: leave the live connection untouched
        }
    }
    plan
}
```

在 `impl GatewayState` 内（`disabled_arc` 之后）加：

```rust
    /// Reconcile the upstream registry against a new config: drop removed upstreams, (re)connect
    /// added/changed ones (reusing the eager connect path), then rebuild the snapshot. Best-effort:
    /// a connect failure is recorded in `connect_failures` and never aborts the others or rolls back.
    pub async fn reconcile_upstreams(
        &self,
        old: &[config::UpstreamConfig],
        new: &[config::UpstreamConfig],
        trigger: upstream::connection::RebuildTrigger,
    ) -> ReconcileSummary {
        let plan = plan_upstream_reconcile(old, new);
        for name in &plan.removed {
            self.registry.remove(name);
        }
        let mut connect_failures = Vec::new();
        if !plan.to_connect.is_empty() {
            let csum =
                upstream::connect::connect_all(self.registry(), &plan.to_connect, trigger).await;
            connect_failures = csum.skipped;
        }
        let _ = self.rebuild_snapshot().await;
        ReconcileSummary {
            added: plan.added,
            removed: plan.removed,
            reconnected: plan.reconnected,
            connect_failures,
        }
    }
```

在 `lib.rs` 的 `#[cfg(test)] mod tests` 内加纯函数测试（`self.registry` 是私有字段，方法内可访问；`registry()` getter 返回 `&UpstreamRegistry`）：

```rust
    fn ups(toml: &str) -> Vec<config::UpstreamConfig> {
        config::Config::from_toml_str(toml).unwrap().upstreams
    }

    #[test]
    fn plan_reconcile_classifies_add_remove_change_unchanged() {
        let a = ups("[[upstream]]\nname=\"keep\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
                     [[upstream]]\nname=\"drop\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
                     [[upstream]]\nname=\"chg\"\ntransport=\"stdio\"\ncommand=\"x\"\n");
        let b = ups("[[upstream]]\nname=\"keep\"\ntransport=\"stdio\"\ncommand=\"x\"\n\
                     [[upstream]]\nname=\"chg\"\ntransport=\"stdio\"\ncommand=\"y\"\n\
                     [[upstream]]\nname=\"new\"\ntransport=\"stdio\"\ncommand=\"x\"\n");
        let p = super::plan_upstream_reconcile(&a, &b);
        assert_eq!(p.removed, vec!["drop"]);
        assert_eq!(p.added, vec!["new"]);
        assert_eq!(p.reconnected, vec!["chg"]);
        // unchanged "keep" is neither removed nor reconnected nor in to_connect
        let tc: Vec<&str> = p.to_connect.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(tc, vec!["chg", "new"]); // order: new list order
        assert!(!tc.contains(&"keep"));
    }
```

- [ ] **Step 3: 写集成测试 `crates/gateway/tests/reconcile.rs`**

```rust
use std::sync::Arc;

use gateway::GatewayState;
use metatools::search_tools;
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

fn ups(toml: &str) -> Vec<config::UpstreamConfig> {
    config::Config::from_toml_str(toml).unwrap().upstreams
}

fn trig() -> tokio::sync::mpsc::Sender<String> {
    tokio::sync::mpsc::channel::<String>(8).0
}

#[tokio::test]
async fn reconcile_removes_deleted_upstream_and_rebuilds() {
    let state = GatewayState::new("bm25").unwrap();
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
    assert!(search_tools(&state.snapshot(), "echo", 5)
        .await
        .iter()
        .any(|s| s.name == "mock__echo"));

    // old has "mock" (command irrelevant — removal is by name), new is empty.
    let old = ups("[[upstream]]\nname=\"mock\"\ntransport=\"stdio\"\ncommand=\"x\"\n");
    let summary = state.reconcile_upstreams(&old, &[], trig()).await;
    assert_eq!(summary.removed, vec!["mock"]);
    assert!(search_tools(&state.snapshot(), "echo", 5).await.is_empty());
    assert!(state.registry().get("mock").is_none());

    join.abort();
}

#[tokio::test]
async fn reconcile_noop_when_unchanged_keeps_connection() {
    let state = GatewayState::new("bm25").unwrap();
    let (handle, join) = connect_mock("mock").await;
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();

    let cfg = ups("[[upstream]]\nname=\"mock\"\ntransport=\"stdio\"\ncommand=\"x\"\n");
    let summary = state.reconcile_upstreams(&cfg, &cfg, trig()).await; // identical -> no-op
    assert!(summary.removed.is_empty() && summary.added.is_empty() && summary.reconnected.is_empty());
    // connection preserved -> tools still searchable after the rebuild
    assert!(search_tools(&state.snapshot(), "echo", 5)
        .await
        .iter()
        .any(|s| s.name == "mock__echo"));

    join.abort();
}

#[tokio::test]
async fn reconcile_add_failure_is_best_effort() {
    // A brand-new upstream whose stdio command can't spawn: connect fails, recorded in
    // connect_failures, no panic, rebuild still runs.
    let state = GatewayState::new("bm25").unwrap();
    let new = ups("[[upstream]]\nname=\"bad\"\ntransport=\"stdio\"\ncommand=\"/nonexistent-mcpgw-bin\"\n");
    let summary = state.reconcile_upstreams(&[], &new, trig()).await;
    assert_eq!(summary.added, vec!["bad"]);
    assert_eq!(summary.connect_failures.len(), 1);
    assert_eq!(summary.connect_failures[0].0, "bad");
    assert!(state.registry().get("bad").is_none()); // failed connect not inserted
}
```

- [ ] **Step 4: 跑测试，先确认编译失败**

Run: `cargo test -p gateway --test reconcile`
Expected: 编译错误 `no method named reconcile_upstreams`。

- [ ] **Step 5: （已在 Step 2 实现）跑测试 + 门禁**

```bash
cargo test -p gateway reconcile
cargo test -p gateway --test reconcile
cargo test -p gateway   # 既有不回归
cargo fmt --all --check && cargo clippy -p gateway --all-targets --all-features -- -D warnings
```
Expected: 纯函数测试 + 3 个集成测试过；既有不回归；clippy 净。

- [ ] **Step 6: Commit**

```bash
git add crates/gateway/Cargo.toml crates/gateway/src/lib.rs crates/gateway/tests/reconcile.rs Cargo.lock
git commit -m "feat(gateway): reconcile_upstreams 三向 diff + best-effort 热重载（复用 connect_all）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: `AppState` 6 新字段 + main.rs 接线（dashboard + mcpgw）

**Files:**
- Modify: `crates/dashboard/src/api.rs`（AppState 加 6 字段；更新测试 `make_state`）
- Modify: `crates/mcpgw/src/main.rs`（`run_serve` 加 `config_path` 参数；`prepare_state` 返回 trigger；构造 validator；AppState 接线）

纯接线（无新行为，handler 在 Task 3/4）。验证 = 编译通过 + 既有测试不回归 + `cargo build`。AppState 派生 `Clone`，6 字段均 `Clone`；`pub` 字段未读不会被 clippy 标记。

- [ ] **Step 1: AppState 加字段（`api.rs`，`admin_token` 之后）**

```rust
    /// Path to the live config file (Some only when `serve --config X`). None -> config edit 404.
    pub config_path: Option<PathBuf>,
    /// Validates candidate TOML (structure + all env refs resolvable) -> Config or error message.
    /// Injected by main.rs so env-resolution stays in the bin (no dashboard->main dependency).
    pub config_validator: std::sync::Arc<dyn Fn(&str) -> Result<config::Config, String> + Send + Sync>,
    /// Serializes config PUTs (validate + write + reconcile).
    pub config_write_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// Boot config snapshot; baseline for the "needs restart" diff of non-upstream sections.
    pub boot_config: std::sync::Arc<config::Config>,
    /// Upstream configs currently applied (reconcile baseline; updated on each successful PUT).
    pub applied_upstreams: std::sync::Arc<std::sync::Mutex<Vec<config::UpstreamConfig>>>,
    /// Rebuild trigger handed to connect_all during upstream hot-reload.
    pub rebuild_trigger: tokio::sync::mpsc::Sender<String>,
```

- [ ] **Step 2: 更新测试构造器 `make_state`（`api.rs` 测试模块）**

在 `make_state` 的 AppState 字面量里（`admin_token: None,` 之后）加：

```rust
            config_path: None,
            config_validator: std::sync::Arc::new(|t: &str| {
                config::Config::from_toml_str(t).map_err(|e| e.to_string())
            }),
            config_write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            boot_config: std::sync::Arc::new(config::Config::default_from_empty()),
            applied_upstreams: std::sync::Arc::new(std::sync::Mutex::new(vec![])),
            rebuild_trigger: tokio::sync::mpsc::channel::<String>(1).0,
```

- [ ] **Step 3: `prepare_state` 返回 rebuild trigger（`main.rs`）**

把 `prepare_state` 的返回类型与体改为也返回一个 trigger clone：

返回类型签名（在 `tokio::sync::mpsc::Receiver<String>,` 之后加一行）：
```rust
        tokio::sync::mpsc::Sender<String>,
```
体内把 channel 段改为：
```rust
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let trigger = tx.clone();
    let csum = upstream::connect::connect_all(state.registry(), &cfg.upstreams, tx).await;
    tracing::info!(connected = ?csum.connected, skipped = ?csum.skipped, "upstreams connected");
    let rsum = state.rebuild_snapshot().await.map_err(|e| e.to_string())?;
    tracing::info!(ingested = ?rsum.ingested, skipped = ?rsum.skipped, "initial snapshot built");
    Ok((state, rx, trigger))
```

- [ ] **Step 4: `run_serve` 加 `config_path` 参数 + 接收 trigger（`main.rs`）**

签名：`async fn run_serve(cfg: config::Config, config_path: Option<PathBuf>) -> Result<(), String> {`
调用点（约 line 109）：`rt.block_on(run_serve(cfg, cli.config.clone()))?;`
`prepare_state` 调用处：`let (state, rx, rebuild_trigger) = prepare_state(&cfg).await?;`

- [ ] **Step 5: 定义具名校验函数 + 单测（`main.rs`，紧邻 `build_backends`）**

```rust
/// Validate candidate config TOML for the dashboard's online editor: structure + every env
/// reference resolvable (same checks `serve` runs at startup, so a saved config can't reference a
/// missing secret). Returns the parsed Config or an error message.
fn validate_config_text(cfg_text: &str) -> Result<config::Config, String> {
    let cfg = config::Config::from_toml_str(cfg_text).map_err(|e| e.to_string())?;
    resolve_api_keys(&cfg)?;
    resolve_admin_token(&cfg)?;
    validate_upstream_http_env(&cfg)?;
    build_backends(&cfg)?;
    Ok(cfg)
}
```

在 `main.rs` 测试模块加：

```rust
    #[test]
    fn validate_config_text_ok_and_rejects_bad_toml_and_missing_env() {
        // structurally valid + no env refs -> Ok
        assert!(validate_config_text("[retrieval]\nstrategy = \"bm25\"\n").is_ok());
        // bad TOML -> Err
        assert!(validate_config_text("not = = toml").is_err());
        // references an unset env (admin_token_env) -> Err (strict: blocks before persist)
        std::env::remove_var("MCPGW_VCT_MISSING");
        let r = validate_config_text(
            "[dashboard]\nenabled = true\nadmin_token_env = \"MCPGW_VCT_MISSING\"\n",
        );
        assert!(r.is_err(), "missing admin_token env must fail validation");
    }
```

- [ ] **Step 6: AppState 接线（`main.rs` 的 `if let Some(listener) = dash_listener` 块内）**

AppState 字面量里（`admin_token: admin_token.clone(),` 之后）加：
```rust
            config_path: config_path.clone(),
            config_validator: std::sync::Arc::new(validate_config_text)
                as std::sync::Arc<dyn Fn(&str) -> Result<config::Config, String> + Send + Sync>,
            config_write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            boot_config: std::sync::Arc::new(cfg.clone()),
            applied_upstreams: std::sync::Arc::new(std::sync::Mutex::new(cfg.upstreams.clone())),
            rebuild_trigger: rebuild_trigger.clone(),
```
（`as Arc<dyn Fn…>` 显式 unsize 把 fn-item 强制到 trait object，最稳。）

- [ ] **Step 7: 跑门禁**

```bash
cargo build --locked
cargo test -p dashboard && cargo test -p mcpgw
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings
```
Expected: 全工作区编译；`validate_config_text` 测试过；dashboard/mcpgw 既有不回归；clippy 净。`rebuild_trigger`/`validate_config_text` 被 AppState 读取 → 无 unused 告警。

- [ ] **Step 8: Commit**

```bash
git add crates/dashboard/src/api.rs crates/mcpgw/src/main.rs
git commit -m "feat(dashboard,mcpgw): AppState 加在线改配 6 字段 + main 注入校验器/trigger/baseline

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: `GET /api/admin/config`（dashboard crate）

**Files:**
- Create: `crates/dashboard/src/admin_config.rs`（`ConfigView` + `get_config` + 测试）
- Modify: `crates/dashboard/src/lib.rs`（`mod admin_config`；admin 子路由加 `GET /api/admin/config`）

读 `config_path` 当前文本原样返回（token-gated；文件无明文密钥）。`config_path == None` → 404。

- [ ] **Step 1: 写测试（`admin_config.rs` 测试模块，复用 `crate::api` 的 `seeded_state`）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::seeded_state;

    #[tokio::test]
    async fn get_config_404_without_path() {
        let st = std::sync::Arc::new(seeded_state().await); // config_path: None
        let r = get_config(State(st)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_config_returns_file_content() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-get-{}.toml", std::process::id()));
        std::fs::write(&p, "[retrieval]\nstrategy = \"bm25\"\n").unwrap();
        let mut state = seeded_state().await;
        state.config_path = Some(p.clone());
        let r = get_config(State(std::sync::Arc::new(state))).await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["content"], "[retrieval]\nstrategy = \"bm25\"\n");
        let _ = std::fs::remove_file(&p);
    }
}
```

> 注：`seeded_state` 现在 `api.rs` 的 `#[cfg(test)] mod tests` 内、是私有 fn。为让 `admin_config.rs` 测试复用，把 `api.rs` 的 `mod tests` 改为 `pub(crate) mod tests` 且 `seeded_state`/`make_state` 改 `pub(crate)`（仅可见性放宽，不改逻辑）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard get_config`
Expected: 编译错误（`admin_config` 模块/`get_config` 不存在）。

- [ ] **Step 3: 写 `admin_config.rs`（实现）**

```rust
//! Online config edit subsystem (M5): GET/PUT the live `mcpgw.toml`, Bearer-gated (mounted on the
//! M4 admin sub-router). GET returns the current file text; PUT (Task 4) validates + persists +
//! hot-reloads upstreams.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::api::AppState;

#[derive(Serialize)]
pub struct ConfigView {
    pub path: String,
    pub content: String,
}

/// `GET /api/admin/config` — current config file text. 404 when serve was started without `--config`.
pub async fn get_config(State(s): State<Arc<AppState>>) -> Response {
    let Some(path) = s.config_path.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match std::fs::read_to_string(path) {
        Ok(content) => Json(ConfigView {
            path: path.display().to_string(),
            content,
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("read config: {e}")).into_response(),
    }
}
```

- [ ] **Step 4: 挂载（`lib.rs`）**

`mod` 区加 `mod admin_config;`。admin 子路由（`.route_layer(...)` 之前）加：
```rust
        .route("/api/admin/config", get(admin_config::get_config))
```
（注意 admin 子路由现在同时用 `get` 与 `post` —— `use axum::routing::{get, post};` 已在 Task 5/M4 引入。）

- [ ] **Step 5: 跑测试 + 门禁**

```bash
cargo test -p dashboard
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings
```
Expected: 2 个 get_config 测试过；既有不回归。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/src/admin_config.rs crates/dashboard/src/lib.rs crates/dashboard/src/api.rs
git commit -m "feat(dashboard): GET /api/admin/config（Bearer，返回当前配置文本）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4: `PUT /api/admin/config` + 原子写 + needs_restart（dashboard crate）

**Files:**
- Modify: `crates/dashboard/src/admin_config.rs`（`ApplyResult` + `restart_diff` + `atomic_write` + `put_config` + 测试）
- Modify: `crates/dashboard/src/lib.rs`（config 路由加 `.put(...)`）

校验（注入器，结构+全 env）→ 原子落盘(+.bak，经 `spawn_blocking` 离开 async worker)→ `reconcile_upstreams` → `needs_restart` diff → `ApplyResult`。全程持 `config_write_lock`。

> 注（Task 1 审查传导）：`ReconcileSummary` 的 `added`/`reconnected` 是**意图**，与 `connect_failures` 交叉看才是真正生效的集合（changed 上游若 reconnect 失败，仍保留旧连接）。`ApplyResult` 按 spec 即 `ReconcileSummary + needs_restart`，**不**再额外折叠 ingest 健康——某上游"连上但 ingest 失败/超时"经既有 Overview/Upstreams 视图（`last_summary`）观察，不在本 PUT 响应里重复。

- [ ] **Step 1: 写测试（`admin_config.rs` 测试模块，追加）**

```rust
    #[tokio::test]
    async fn put_config_404_without_path() {
        let st = std::sync::Arc::new(seeded_state().await);
        let r = put_config(State(st), "[retrieval]\nstrategy=\"bm25\"\n".into()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_config_400_on_invalid_toml_does_not_write() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-bad-{}.toml", std::process::id()));
        std::fs::write(&p, "[retrieval]\nstrategy = \"bm25\"\n").unwrap();
        let mut state = seeded_state().await;
        state.config_path = Some(p.clone());
        let r = put_config(State(std::sync::Arc::new(state)), "not = = toml".into()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // file untouched
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "[retrieval]\nstrategy = \"bm25\"\n");
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn put_config_persists_with_bak_and_reports_needs_restart() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-put-{}.toml", std::process::id()));
        let old = "[retrieval]\nstrategy = \"bm25\"\n";
        std::fs::write(&p, old).unwrap();
        let mut state = seeded_state().await; // boot_config = default (dashboard.enabled = false)
        state.config_path = Some(p.clone());
        let st = std::sync::Arc::new(state);

        let new = "[dashboard]\nenabled = true\n"; // differs from boot in [dashboard] -> needs restart
        let r = put_config(State(st), new.to_string()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), new); // persisted verbatim

        let mut bak = p.clone().into_os_string();
        bak.push(".bak");
        assert_eq!(std::fs::read_to_string(std::path::PathBuf::from(bak)).unwrap(), old); // .bak = old

        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let nr: Vec<String> = serde_json::from_value(v["needs_restart"].clone()).unwrap();
        assert!(nr.contains(&"dashboard".to_string()), "got {nr:?}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn atomic_write_creates_bak_and_leaves_no_temp() {
        let p = std::env::temp_dir().join(format!("mcpgw-cfg-aw-{}.toml", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        atomic_write(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let mut bak = p.clone().into_os_string();
        bak.push(".bak");
        assert_eq!(std::fs::read_to_string(std::path::PathBuf::from(&bak)).unwrap(), "old");
        // no temp left
        let mut tmp = p.clone().into_os_string();
        tmp.push(format!(".tmp.{}", std::process::id()));
        assert!(!std::path::PathBuf::from(tmp).exists());
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(std::path::PathBuf::from(bak));
    }

    #[tokio::test]
    async fn config_routes_are_gated() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        for method in ["GET", "PUT"] {
            let st = std::sync::Arc::new(seeded_state().await); // admin_token None
            let r = crate::build_dashboard_router(st, false)
                .oneshot(Request::builder().method(method).uri("/api/admin/config").body(Body::empty()).unwrap())
                .await.unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{method} unconfigured -> 404");
        }
        for method in ["GET", "PUT"] {
            let mut s = seeded_state().await;
            s.admin_token = Some(std::sync::Arc::from("sek"));
            let r = crate::build_dashboard_router(std::sync::Arc::new(s), false)
                .oneshot(Request::builder().method(method).uri("/api/admin/config").body(Body::empty()).unwrap())
                .await.unwrap();
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{method} no-bearer -> 401");
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard put_config`
Expected: 编译错误（`put_config`/`atomic_write` 不存在）。

- [ ] **Step 3: 实现（`admin_config.rs` 追加；顶部 `use` 加 `std::io::Write` + `std::path::Path`）**

```rust
#[derive(serde::Serialize)]
pub struct ApplyResult {
    pub upstreams: gateway::ReconcileSummary,
    pub needs_restart: Vec<&'static str>,
}

/// Non-upstream sections of `new` that differ from the boot baseline → need a restart to take effect.
fn restart_diff(boot: &config::Config, new: &config::Config) -> Vec<&'static str> {
    let mut v = Vec::new();
    if boot.retrieval != new.retrieval {
        v.push("retrieval");
    }
    if boot.server != new.server {
        v.push("server");
    }
    if boot.audit != new.audit {
        v.push("audit");
    }
    if boot.dashboard != new.dashboard {
        v.push("dashboard");
    }
    v
}

/// Best-effort atomic write: backup current to `<path>.bak`, then temp → fsync → rename.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        if let Err(e) = std::fs::copy(path, std::path::PathBuf::from(&bak)) {
            tracing::warn!(path = %path.display(), error = %e, "config .bak backup failed");
        }
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// `PUT /api/admin/config` — validate (structure + env) → atomic persist(+.bak) → hot-reload
/// upstreams → report reconcile result + needs-restart sections.
pub async fn put_config(State(s): State<Arc<AppState>>, body: String) -> Response {
    let Some(path) = s.config_path.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let _guard = s.config_write_lock.lock().await; // serialize config writes

    let new_cfg = match (s.config_validator)(&body) {
        Ok(c) => c,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };

    let w_path = path.clone();
    let w_body = body.clone();
    let write_res = tokio::task::spawn_blocking(move || atomic_write(&w_path, &w_body))
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
    if let Err(e) = write_res {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("persist config: {e}")).into_response();
    }

    let old_ups = s.applied_upstreams.lock().unwrap().clone();
    let summary = s
        .gateway
        .reconcile_upstreams(&old_ups, &new_cfg.upstreams, s.rebuild_trigger.clone())
        .await;
    *s.applied_upstreams.lock().unwrap() = new_cfg.upstreams.clone();

    let needs_restart = restart_diff(&s.boot_config, &new_cfg);
    Json(ApplyResult {
        upstreams: summary,
        needs_restart,
    })
    .into_response()
}
```

- [ ] **Step 4: 路由加 PUT（`lib.rs`）**

把 `.route("/api/admin/config", get(admin_config::get_config))` 改为：
```rust
        .route(
            "/api/admin/config",
            get(admin_config::get_config).put(admin_config::put_config),
        )
```

- [ ] **Step 5: 跑测试 + 门禁**

```bash
cargo test -p dashboard
cargo fmt --all --check && cargo clippy -p dashboard --all-targets --all-features -- -D warnings
cargo build --locked
```
Expected: 5 个新测试过；既有不回归；clippy 净。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/src/admin_config.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): PUT /api/admin/config——严格校验+原子写(.bak)+上游热重载+needs_restart

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: 前端——Config 编辑器（dashboard UI）

**Files:**
- Modify: `crates/dashboard/ui/src/lib/admin.svelte.js`（加 `adminGet`/`adminPut`）
- Create: `crates/dashboard/ui/src/lib/Config.svelte`
- Modify: `crates/dashboard/ui/src/lib/Nav.svelte`（token 在时加 Config 项）、`crates/dashboard/ui/src/App.svelte`（路由）、`crates/dashboard/ui/src/app.css`（`.cfg-edit`）
- 重建 `crates/dashboard/ui/dist/`

> 前端无测试框架：验证 = `npm run build` 通过 + dist 再生 + `cargo build -p dashboard --locked` 内嵌。
>
> 注（Task 1 审查传导）：结果展示里 `+added −removed ~reconnected` 是**尝试**计数；`connect_failures` 用 `badge error` 单独突出（已在 Config.svelte 实现），运维据此区分"已生效"与"尝试但失败（保留旧连接）"。

- [ ] **Step 1: `admin.svelte.js` 加 GET/PUT 助手**

```js
/** GET an admin endpoint with the in-memory Bearer token. Returns the raw Response. */
export function adminGet(path) {
  return fetch(path, { headers: admin.token ? { Authorization: `Bearer ${admin.token}` } : {} });
}

/** PUT a text body to an admin endpoint with the Bearer token. Returns the raw Response. */
export function adminPut(path, body) {
  return fetch(path, {
    method: "PUT",
    headers: admin.token ? { Authorization: `Bearer ${admin.token}` } : {},
    body,
  });
}
```

- [ ] **Step 2: 创建 `Config.svelte`**

```svelte
<script>
  import { admin, adminGet, adminPut } from "./admin.svelte.js";
  let content = $state("");
  let loaded = $state(false);
  let error = $state(null);
  let result = $state(null);
  let busy = $state(false);

  async function load() {
    error = null; result = null;
    try {
      const r = await adminGet("/api/admin/config");
      if (r.status === 404) { error = "serve 未带 --config（无文件可改）"; loaded = false; return; }
      if (r.status === 401) { error = "admin token 失效，请在 About 重新输入"; loaded = false; return; }
      if (!r.ok) { error = `GET -> ${r.status}`; loaded = false; return; }
      content = (await r.json()).content; loaded = true;
    } catch (e) { error = String(e); }
  }

  async function save() {
    busy = true; error = null; result = null;
    try {
      const r = await adminPut("/api/admin/config", content);
      if (r.status === 200) result = await r.json();
      else error = `${r.status}: ${await r.text()}`;
    } catch (e) { error = String(e); }
    finally { busy = false; }
  }

  $effect(() => { if (admin.token && !loaded) load(); });
</script>

<h2>Config</h2>
{#if !admin.token}
  <p class="muted">需要 admin token（在 About 页输入）才能编辑配置。</p>
{:else}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if loaded}
    <textarea class="cfg-edit" bind:value={content} spellcheck="false" aria-label="config TOML"></textarea>
    <div class="toolbar">
      <button class="admbtn" onclick={save} disabled={busy}>{busy ? "saving…" : "Save"}</button>
      <button class="admbtn" onclick={load} disabled={busy}>Reload</button>
    </div>
    {#if result}
      <div class="card" style="margin-top:10px">
        <p>✓ saved · upstreams +{result.upstreams.added.length} −{result.upstreams.removed.length} ~{result.upstreams.reconnected.length}
          {#if result.upstreams.connect_failures.length}
            <span class="badge error">connect failed: {result.upstreams.connect_failures.map((f) => f[0]).join(", ")}</span>
          {/if}
        </p>
        {#if result.needs_restart.length}
          <p><span class="badge skipped">需重启生效</span> {result.needs_restart.join(", ")}</p>
        {/if}
      </div>
    {/if}
  {/if}
{/if}
```

- [ ] **Step 3: `Nav.svelte` —— token 在时加 Config 项**

`<script>` 加 `import { admin } from "./admin.svelte.js";`；把 `const items = [...]` 改为基集 + 派生：
```js
  const base = [
    ["overview", "Overview", "overview"],
    ["upstreams", "Upstreams", "upstreams"],
    ["tools", "Tools", "tools"],
    ["calls", "Calls", "calls"],
    ["traces", "Traces", "traces"],
    ["about", "About", "info"],
  ];
  const items = $derived(admin.token ? [...base, ["config", "Config", "wrench"]] : base);
```

- [ ] **Step 4: `App.svelte` —— 加路由**

`<script>` 加 `import Config from "./lib/Config.svelte";`；在 `{:else if route.view === "about"}` 之后加：
```svelte
    {:else if route.view === "config"}
      <Config />
```

- [ ] **Step 5: `app.css` —— `.cfg-edit` 样式（文件末尾）**

```css
.cfg-edit {
  width: 100%;
  min-height: 360px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 13px;
  line-height: 1.5;
  padding: 10px 12px;
  color: var(--fg);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 8px;
  resize: vertical;
}
```

- [ ] **Step 6: 构建并验证**

```bash
cd crates/dashboard/ui && npm run build && cd -
git status --short crates/dashboard/ui/dist
cargo build -p dashboard --locked
cargo test -p dashboard   # rust-embed/XSS-guard 测试仍绿
```
Expected: 构建成功；dist 再生；cargo build/test 绿。

- [ ] **Step 7: Commit**

```bash
git add crates/dashboard/ui/src/lib/admin.svelte.js crates/dashboard/ui/src/lib/Config.svelte crates/dashboard/ui/src/lib/Nav.svelte crates/dashboard/ui/src/App.svelte crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): Config 编辑器（GET/PUT /api/admin/config，token-gated）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: 文档 L1–L4 同步 + .gitignore

**Files:**
- Modify: `.gitignore`
- Modify: `docs/L1-overview.md`、`docs/L2-components/{dashboard,gateway}.md`、`docs/L3-details/{dashboard,gateway}.md`、`docs/L4-api/{dashboard,mcpgw-main}.md`

把"在线改配 + 上游热重载（写子系统 C）"贯穿四层；端点 18 → 20。

- [ ] **Step 1: `.gitignore` 加备份文件**

追加一行：
```
*.bak
```

- [ ] **Step 2: L1-overview.md**

- dashboard 段：补"**写子系统 C：在线改配 + 上游热重载**——`GET/PUT /api/admin/config`（Bearer），严格校验
  （结构 + 全 env 引用）后原子写(+.bak)，`[[upstream]]` 增/删/改热重载、其余字段需重启"；端点 **18 → 20**
  （ASCII 框 + 能力列表 + 计数处一致更新）。
- gateway 段：补"`reconcile_upstreams` 三向 diff 协调 registry + rebuild"。
- L1 测试计数更新（跑 `cargo test --all-features` 取实际数）。

- [ ] **Step 3: L2-components/dashboard.md**

- 端点 18 → 20，列出 `GET/PUT /api/admin/config`（Bearer）。
- 公开接口加 `admin_config.rs`（`get_config`/`put_config`/`atomic_write`/`restart_diff`/`ApplyResult`/`ConfigView`）。
- "不负责"段：澄清现在做"运行时禁用 + **在线改配/上游热重载**"两类写；仍不重启进程、不做 top_k/strategy 热重载。
- AppState 新增 6 字段说明（config_path/config_validator/config_write_lock/boot_config/applied_upstreams/rebuild_trigger）。

- [ ] **Step 4: L3-details/dashboard.md**

补"在线改配子系统"小节：严格校验链（结构 + resolve_*）/ 原子写 temp→fsync→rename + .bak（best-effort，
spawn_blocking 离开 async worker）/ `reconcile_upstreams` 调用 / `needs_restart` 基线 = boot_config / 写锁串行 /
config_path=None→404 / 校验失败→400 不写盘 / 明文 token 的部署前提（同 M4 注记）。

- [ ] **Step 5: L2/L3 gateway.md**

- L2：`GatewayState::reconcile_upstreams(old, new, trigger) -> ReconcileSummary` 接口行 + `ReconcileSummary` 类型。
- L3：三向 diff 算法（removed→remove / added·changed→connect_all / unchanged→不动）+ best-effort + 末尾 rebuild
  + 与 M4 禁用过滤的组合。

- [ ] **Step 6: L4 dashboard.md + mcpgw-main.md**

- L4 dashboard：`GET/PUT /api/admin/config` 端点 + `admin_config` 各符号；端点表 +2（18→20）。
- L4 mcpgw-main：`validate_config_text` + `run_serve(config_path)` + `prepare_state` 返回 trigger +
  AppState 6 字段注入。

- [ ] **Step 7: 一致性校验 + Commit**

```bash
grep -rn "18 个\|20 个\|/api/admin/config" docs/L1-overview.md docs/L2-components/dashboard.md docs/L4-api/dashboard.md
cargo build --locked
```
Expected: 端点计数处处一致为 20；`/api/admin/config` 在 L1/L2/L4 出现。

```bash
git add .gitignore docs/
git commit -m "docs: L1–L4 同步在线改配 + 上游热重载（写子系统 C，端点 18→20）

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 自检清单（实现完成后）

- [ ] 全门禁绿：`cargo fmt --all --check` / `cargo clippy --all-targets --all-features -D warnings` / `cargo test --all-features` / `cargo build --locked`。
- [ ] `npm run build` 可复现、`dist/` 入库、`cargo build` node-free。
- [ ] 端点计数 20 在 L1/L2/L4 一致。
- [ ] 默认不配置 `admin_token_env`/`--config` 时：`/api/admin/config` 全 404，行为零变化。
- [ ] 校验失败不写盘；原子写有 `.bak`；密钥仅 env 引用、绝不入日志。
- [ ] 上游热重载与 M4 禁用集正确组合（热加的禁用上游仍隐藏）。
