# Dashboard About / Settings 视图实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 给只读 dashboard 加 About/Settings 页：`GET /api/about` 返回启动时组装的非敏感配置/限额 + 版本/构建信息，前端新 `About.svelte` 分组展示。

**Architecture:** dashboard crate 新增 `about.rs`（`AboutInfo` 类型 + `AboutInfo::from_config(&config::Config, VersionInfo)` 纯函数 + 隐私单测）；`crates/mcpgw/build.rs` 取 git SHA/构建时间写入 env，`main.rs` 组 `VersionInfo` 并把 `AboutInfo` 存进 `AppState.about`；`/api/about` 路由序列化它。前端 `About.svelte` + 导航/路由/图标。后端只读、仅非敏感、绝不含密钥/env 值。

**Tech Stack:** Rust（axum + serde, dashboard crate；mcpgw build.rs）、Svelte 5 runes + Vite。

参考 spec：`docs/superpowers/specs/2026-06-22-mcpgw-dashboard-about-view-design.md`

---

## 文件结构

- **Create** `crates/dashboard/src/about.rs` —— `AboutInfo` 及嵌套类型 + `from_config` + `transport_label` + 单测。
- **Modify** `crates/dashboard/src/lib.rs` —— `mod about; pub use about::{AboutInfo, VersionInfo};` + `h_about` handler + `/api/about` 路由。
- **Modify** `crates/dashboard/src/api.rs` —— `AppState` 加 `pub about: AboutInfo;` + `pub fn about(&AppState) -> AboutInfo`（或直接 handler 序列化 `state.about`）。
- **Create** `crates/mcpgw/build.rs` —— git SHA + 构建时间写 env。
- **Modify** `crates/mcpgw/src/main.rs` —— 组 `VersionInfo` + `AppState.about` 字段。
- **Create** `crates/dashboard/ui/src/lib/About.svelte`。
- **Modify** `crates/dashboard/ui/src/lib/Nav.svelte`、`crates/dashboard/ui/src/lib/Icon.svelte`、`crates/dashboard/ui/src/App.svelte`。
- **Modify** `crates/mcpgw/tests/dashboard.rs`、`docs/L1-overview.md`、`docs/L2-components/dashboard.md`、`docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`、`docs/L4-api/mcpgw-main.md`。

---

## Task 1：后端 `about.rs` —— 类型 + `from_config` + 隐私单测

**Files:**
- Create: `crates/dashboard/src/about.rs`
- Modify: `crates/dashboard/src/lib.rs`（仅加 `mod about;` 让其编译/测试；`pub use` 与路由留到 Task 2）

- [ ] **Step 1: 在 `lib.rs` 注册模块（仅编译用）**

在 `crates/dashboard/src/lib.rs` 的 `mod activity;` 那行之后加：

```rust
mod about;
```

- [ ] **Step 2: 创建 `about.rs`（类型 + `from_config` 骨架 + 失败测试）**

```rust
//! About/Settings 只读视图的数据形状：启动时从 `config::Config` 组装的**非敏感**生效配置/限额 + 版本。
//! 隐私上 `AboutInfo` 及其嵌套类型**字段集里根本不含**任何密钥/token/env 名/env 值。

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct AboutInfo {
    pub version: VersionInfo,
    pub retrieval: RetrievalInfo,
    pub dashboard: DashboardInfo,
    pub audit: AuditInfo,
    pub server: ServerInfo,
    pub upstreams: Vec<UpstreamConfigInfo>,
}

#[derive(Serialize, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub git_sha: String,
    pub build_time: String,
}

#[derive(Serialize, Clone)]
pub struct RetrievalInfo {
    pub strategy: String,
    pub top_k: usize,
}

#[derive(Serialize, Clone)]
pub struct DashboardInfo {
    pub call_buffer: usize,
    pub payload_max_bytes: usize,
    pub trace_queries: bool,
    pub trace_buffer: usize,
    pub trace_path: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct AuditInfo {
    pub enabled: bool,
    pub path: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct ServerInfo {
    pub stdio: bool,
    pub http_enabled: bool,
    pub http_bind: Option<String>,
    pub http_path: Option<String>,
    pub http_auth: bool,
}

#[derive(Serialize, Clone)]
pub struct UpstreamConfigInfo {
    pub name: String,
    pub transport: String,
    pub call_timeout_ms: u64,
}

/// 上游 transport → 短标签（自包含，避免 dashboard 反向依赖 mcpgw 的 `transport_str`）。
fn transport_label(t: &config::UpstreamTransport) -> &'static str {
    match t {
        config::UpstreamTransport::Stdio { .. } => "stdio",
        config::UpstreamTransport::Http { .. } => "http",
    }
}

impl AboutInfo {
    /// 从生效配置 + 版本组装只读 About 视图。仅非敏感字段：绝不含密钥/token/env 名/值/上游认证引用。
    pub fn from_config(cfg: &config::Config, version: VersionInfo) -> AboutInfo {
        let (http_enabled, http_bind, http_path, http_auth) = match &cfg.server.http {
            Some(h) if h.enabled => (
                true,
                Some(h.bind.clone()),
                Some(h.path.clone()),
                !h.api_keys.is_empty(),
            ),
            _ => (false, None, None, false),
        };
        AboutInfo {
            version,
            retrieval: RetrievalInfo {
                strategy: cfg.retrieval.strategy.clone(),
                top_k: cfg.retrieval.top_k,
            },
            dashboard: DashboardInfo {
                call_buffer: cfg.dashboard.call_buffer,
                payload_max_bytes: cfg.dashboard.payload_max_bytes,
                trace_queries: cfg.dashboard.trace_queries,
                trace_buffer: cfg.dashboard.trace_buffer,
                trace_path: cfg.dashboard.trace_path.clone(),
            },
            audit: AuditInfo {
                enabled: cfg.audit.enabled,
                path: cfg.audit.enabled.then(|| cfg.audit.path.clone()),
            },
            server: ServerInfo {
                stdio: cfg.server.stdio,
                http_enabled,
                http_bind,
                http_path,
                http_auth,
            },
            upstreams: cfg
                .upstreams
                .iter()
                .map(|u| UpstreamConfigInfo {
                    name: u.name.clone(),
                    transport: transport_label(&u.transport).to_string(),
                    call_timeout_ms: u.call_timeout_ms,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver() -> VersionInfo {
        VersionInfo { version: "0.1.0".into(), git_sha: "abc123".into(), build_time: "0".into() }
    }

    #[test]
    fn from_config_maps_non_sensitive_fields() {
        let toml = "[retrieval]\nstrategy = \"bm25\"\ntop_k = 7\n\
                    [dashboard]\nenabled = true\ncall_buffer = 1234\npayload_max_bytes = 4096\n\
                    [[upstream]]\nname = \"mock\"\ntransport = \"stdio\"\ncommand = \"x\"\n";
        let cfg = config::Config::from_toml_str(toml).unwrap();
        let a = AboutInfo::from_config(&cfg, ver());
        assert_eq!(a.retrieval.strategy, "bm25");
        assert_eq!(a.retrieval.top_k, 7);
        assert_eq!(a.dashboard.call_buffer, 1234);
        assert_eq!(a.dashboard.payload_max_bytes, 4096);
        assert!(!a.audit.enabled);
        assert_eq!(a.audit.path, None);
        assert!(!a.server.http_enabled);
        assert_eq!(a.upstreams.len(), 1);
        assert_eq!(a.upstreams[0].name, "mock");
        assert_eq!(a.upstreams[0].transport, "stdio");
        assert_eq!(a.version.version, "0.1.0");
    }

    #[test]
    fn http_auth_true_and_no_secrets_leak() {
        let toml = "[retrieval]\nstrategy = \"bm25\"\n\
                    [server.http]\nenabled = true\nbind = \"0.0.0.0:9000\"\npath = \"/mcp\"\n\
                    [[server.http.api_key]]\nname = \"admin\"\nenv = \"SECRET_KEY\"\n\
                    [[upstream]]\nname = \"remote\"\ntransport = \"http\"\nurl = \"https://example.com/mcp\"\nbearer_env = \"REMOTE_TOKEN\"\ncall_timeout_ms = 5000\n";
        let cfg = config::Config::from_toml_str(toml).unwrap();
        let a = AboutInfo::from_config(&cfg, ver());
        assert!(a.server.http_enabled);
        assert!(a.server.http_auth, "api_key present -> auth enabled");
        assert_eq!(a.upstreams[0].transport, "http");
        assert_eq!(a.upstreams[0].call_timeout_ms, 5000);
        let json = serde_json::to_string(&a).unwrap();
        for secret in ["SECRET_KEY", "REMOTE_TOKEN", "admin", "example.com", "bearer_env", "api_key"] {
            assert!(!json.contains(secret), "About JSON must not leak {secret:?}: {json}");
        }
    }
}
```

- [ ] **Step 3: 跑测确认通过（TDD：本 task 的实现已随测试给出，直接验证）**

Run: `cargo test -p dashboard about:: 2>&1 | tail -8`
Expected: `test result: ok. 2 passed`。

- [ ] **Step 4: clippy + fmt**

Run: `cargo clippy -p dashboard --all-targets -- -D warnings 2>&1 | tail -2 && cargo fmt -p dashboard --check`
Expected: clippy 0 警告；fmt 干净（若不干净先 `cargo fmt -p dashboard` 再确认改动文件）。

> 注：本 task 让 `mod about;` 私有但含 `pub` 项，`-D warnings` 下会触发 `dead_code`。若报错，在 `mod about;`
> 上加 `#[allow(dead_code)]` 并注释「Task 2 加 pub use 后移除」（与既有 activity 同样手法）。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/src/about.rs crates/dashboard/src/lib.rs
git commit -m "feat(dashboard): AboutInfo types + from_config (non-sensitive) + privacy tests

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：build.rs + main.rs 接线 + `/api/about` 路由 + e2e + L3/L4 文档

**Files:**
- Create: `crates/mcpgw/build.rs`
- Modify: `crates/dashboard/src/api.rs`、`crates/dashboard/src/lib.rs`、`crates/mcpgw/src/main.rs`
- Modify: `crates/mcpgw/tests/dashboard.rs`、`docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`、`docs/L4-api/mcpgw-main.md`

- [ ] **Step 1: 创建 `crates/mcpgw/build.rs`**

```rust
use std::process::Command;

fn main() {
    // 取构建时短 commit SHA；非 git 仓库/无 git/失败 -> "unknown"（优雅降级）。
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MCPGW_GIT_SHA={sha}");

    // 构建时间（epoch 秒，前端格式化）。不强制每次重跑，故为「最近一次重建」的近似时间。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=MCPGW_BUILD_TIME={ts}");
}
```

- [ ] **Step 2: `api.rs` 的 `AppState` 加 `about` 字段**

在 `crates/dashboard/src/api.rs` 的 `pub struct AppState { ... }` 里、`started_at` 之后加：

```rust
    /// 启动时组装的只读 About/Settings 信息（非敏感）。
    pub about: crate::about::AboutInfo,
```

- [ ] **Step 3: `lib.rs` 暴露类型 + 加 handler + 路由（并移除 Task 1 可能加的 allow）**

3a. 把 Task 1 的 `mod about;`（若加了 `#[allow(dead_code)]` 一并删除该属性+注释）改为：

```rust
mod about;
pub use about::{AboutInfo, VersionInfo};
```

3b. 在 `h_activity` handler 之后加（直接序列化启动时组装好的值，无需 blocking pool）：

```rust
async fn h_about(State(s): State<Arc<AppState>>) -> Json<AboutInfo> {
    Json(s.about.clone())
}
```

3c. 在路由链里、`.route("/api/activity", get(h_activity))` 之后加：

```rust
        .route("/api/about", get(h_about))
```

- [ ] **Step 4: `main.rs` 组 `VersionInfo` 并填 `AppState.about`**

在 `crates/mcpgw/src/main.rs` 的 `AppState { ... }` 字面量里、`started_at: std::time::Instant::now(),` 之后加：

```rust
            about: dashboard::AboutInfo::from_config(
                &cfg,
                dashboard::VersionInfo {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    git_sha: env!("MCPGW_GIT_SHA").to_string(),
                    build_time: env!("MCPGW_BUILD_TIME").to_string(),
                },
            ),
```

- [ ] **Step 5: 编译 + 全 crate 测试 + clippy**

Run: `cargo test -p dashboard 2>&1 | tail -4 && cargo clippy -p dashboard --all-targets -- -D warnings 2>&1 | tail -2 && cargo build -p mcpgw --bin mcpgw 2>&1 | tail -1`
Expected: dashboard 测试全过（含 about:: 2）；clippy 0 警告；mcpgw 编译通过（build.rs 生效、`env!` 解析）。

- [ ] **Step 6: mock 上游 e2e 加 `/api/about` 断言**

在 `crates/mcpgw/tests/dashboard.rs` 的 `dashboard_detail_endpoints_with_mock_upstream` 里、`client.cancel().await.unwrap();` 之前追加（匹配既有 `.send().await.unwrap().json().await.unwrap()` 风格）：

```rust
    // About/Settings: version present, no auth (test config has no api_key), mock upstream listed.
    let about: serde_json::Value = http
        .get(format!("{base}/api/about"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !about["version"]["version"].as_str().unwrap().is_empty(),
        "about.version.version is non-empty"
    );
    assert_eq!(about["server"]["http_auth"], serde_json::json!(false));
    let ups = about["upstreams"].as_array().unwrap();
    assert!(
        ups.iter().any(|u| u["name"] == "mock" && u["transport"] == "stdio"),
        "mock upstream listed: {ups:?}"
    );
```

Run: `cargo build -p upstream --features testkit --bin mock-stdio && MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored 2>&1 | tail -8`
Expected: 2 passed（含 about 断言）。

- [ ] **Step 7: 同步 L3/L4 文档（READ 后改，忠实于代码）**

- `docs/L4-api/dashboard.md`：端点列表加 `GET /api/about` → `AboutInfo`（启动时组装、运行期不变）；记录
  `AboutInfo`/`VersionInfo`/`RetrievalInfo`/`DashboardInfo`/`AuditInfo`/`ServerInfo`/`UpstreamConfigInfo` 各字段、
  `AboutInfo::from_config`、`AppState.about`；强调**仅非敏感、绝不含密钥/env 名/值**（`http_auth` 仅 bool）。
- `docs/L3-details/dashboard.md`：新增「About/Settings」段：启动时 `from_config` 组装一次、运行期不变、零计算；
  **隐私边界**（字段集不含密钥/token/env 引用，单测断言序列化无 `SECRET_KEY`/`bearer_env`/`api_key` 等）；
  `transport_label` 自包含（避免 dashboard 反向依赖 mcpgw）。
- `docs/L4-api/mcpgw-main.md`：记录 `build.rs`（git SHA + 构建时间写 `MCPGW_GIT_SHA`/`MCPGW_BUILD_TIME`、优雅降级、
  不强制每次重跑的语义）+ `main.rs` 组 `VersionInfo` 并把 `AboutInfo::from_config(&cfg, ver)` 填进 `AppState.about`。

- [ ] **Step 8: Commit**

```bash
git add crates/mcpgw/build.rs crates/dashboard/src/api.rs crates/dashboard/src/lib.rs crates/mcpgw/src/main.rs crates/mcpgw/tests/dashboard.rs docs/L3-details/dashboard.md docs/L4-api/dashboard.md docs/L4-api/mcpgw-main.md
git commit -m "feat(dashboard): /api/about endpoint + build.rs version/sha + e2e + docs

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：前端 `About.svelte` + Nav/Icon/App 接入

**Files:**
- Create: `crates/dashboard/ui/src/lib/About.svelte`
- Modify: `crates/dashboard/ui/src/lib/Nav.svelte`、`crates/dashboard/ui/src/lib/Icon.svelte`、`crates/dashboard/ui/src/App.svelte`
- Regenerate: `crates/dashboard/ui/dist/**`

- [ ] **Step 1: 创建 `About.svelte`（挂载拉一次，不轮询；沿用 kv-table/badge/empty/skeleton 样式）**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let info = $state(null);
  let error = $state(null);
  onMount(async () => {
    try { info = await getJSON("/api/about"); }
    catch (e) { error = String(e); }
  });
  function built(secs) {
    const n = Number(secs);
    return n > 0 ? new Date(n * 1000).toLocaleString() : "unknown";
  }
</script>

<h2>About</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if info}
  <h3>Version</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>version</th><td>{info.version.version}</td></tr>
    <tr><th>git</th><td class="mono">{info.version.git_sha}</td></tr>
    <tr><th>built</th><td>{built(info.version.build_time)}</td></tr>
  </tbody></table></div>

  <h3>Retrieval</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>strategy</th><td>{info.retrieval.strategy}</td></tr>
    <tr><th>top_k</th><td>{info.retrieval.top_k}</td></tr>
  </tbody></table></div>

  <h3>Dashboard</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>call_buffer</th><td>{info.dashboard.call_buffer}</td></tr>
    <tr><th>payload_max_bytes</th><td>{info.dashboard.payload_max_bytes}</td></tr>
    <tr><th>trace_queries</th><td>{info.dashboard.trace_queries}</td></tr>
    <tr><th>trace_buffer</th><td>{info.dashboard.trace_buffer}</td></tr>
    <tr><th>trace_path</th><td class="mono">{info.dashboard.trace_path ?? "—"}</td></tr>
  </tbody></table></div>

  <h3>Audit</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>enabled</th><td><span class="badge {info.audit.enabled ? 'ok' : 'unknown'}">{info.audit.enabled ? "on" : "off"}</span></td></tr>
    <tr><th>path</th><td class="mono">{info.audit.path ?? "—"}</td></tr>
  </tbody></table></div>

  <h3>Server</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>stdio</th><td>{info.server.stdio}</td></tr>
    <tr><th>http</th><td><span class="badge {info.server.http_enabled ? 'ok' : 'unknown'}">{info.server.http_enabled ? "enabled" : "disabled"}</span></td></tr>
    <tr><th>http_bind</th><td class="mono">{info.server.http_bind ?? "—"}</td></tr>
    <tr><th>http_path</th><td class="mono">{info.server.http_path ?? "—"}</td></tr>
    <tr><th>http_auth</th><td><span class="badge {info.server.http_auth ? 'ok' : 'unknown'}">{info.server.http_auth ? "enabled" : "disabled"}</span></td></tr>
  </tbody></table></div>

  <h3>Upstreams ({info.upstreams.length})</h3>
  {#if info.upstreams.length === 0}
    <div class="empty"><div>No upstreams configured</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>name</th><th>transport</th><th class="num">timeout_ms</th></tr></thead>
      <tbody>
        {#each info.upstreams as u}
          <tr><td class="mono">{u.name}</td><td>{u.transport}</td><td class="num">{u.call_timeout_ms}</td></tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(6) as _}<div class="sk row"></div>{/each}</div>
{/if}
```

- [ ] **Step 2: `Nav.svelte` 加 About 导航项**

把
```svelte
    ["traces", "Traces", "traces"],
  ];
```
改为
```svelte
    ["traces", "Traces", "traces"],
    ["about", "About", "info"],
  ];
```

- [ ] **Step 3: `Icon.svelte` 加 `info` 图标**

在 `Icon.svelte` 的 `{#if}/{:else if}` 链末尾、最后的 `{/if}` 之前加：

```svelte
  {:else if name === "info"}
    <circle cx="12" cy="12" r="9" /><line x1="12" y1="11" x2="12" y2="16" /><line x1="12" y1="8" x2="12.01" y2="8" />
```

- [ ] **Step 4: `App.svelte` 引入并加路由**

4a. 在组件 import 区（其它页面 import 附近）加：
```svelte
  import About from "./lib/About.svelte";
```

4b. 把路由链里的
```svelte
    {:else if route.view === "traces"}
      <Traces />
    {:else}
      <p class="muted">coming soon</p>
```
改为
```svelte
    {:else if route.view === "traces"}
      <Traces />
    {:else if route.view === "about"}
      <About />
    {:else}
      <p class="muted">coming soon</p>
```

- [ ] **Step 5: 构建 + assets 测试**

Run: `cd crates/dashboard/ui && npm run build 2>&1 | tail -6 && cd ../../.. && cargo test -p dashboard assets:: 2>&1 | grep -E '^test result:'`
Expected: 构建成功、**0 警告**；`assets::` 3 passed。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): About/Settings page + nav/icon/route

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：L1/L2 文档 + 四道门禁 + 计数回填

**Files:**
- Modify: `docs/L1-overview.md`、`docs/L2-components/dashboard.md`

- [ ] **Step 1: L2 文档（`docs/L2-components/dashboard.md`，READ 后改）**

加 `AboutInfo`/`AboutInfo::from_config`/`AppState.about` 与 `GET /api/about` 条目；端点数 **12 → 13**；
一句隐私说明（仅非敏感配置/限额/版本，绝不含密钥/env 值）。

- [ ] **Step 2: L1 文档（`docs/L1-overview.md`，READ 后改）**

- dashboard 端点数 `12` → `13`，端点枚举加 `about`；子系统 A 末尾补一句：
  `+ /api/about About/Settings（启动时组装的非敏感生效配置/限额 + 版本/git SHA/构建时间；前端 About 页）`。
- 测试计数行留待 Step 3 实测回填。

- [ ] **Step 3: 四道门禁 + 计数回填**

```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿（预期较合并基线 +2：新增 `about::` 2 个单测，约 **285 passed / 5 ignored**——以实测为准）。用
`cargo test --all-features 2>&1 | awk '/^test result:/{p+=$4; i+=$8} END{print p" passed / "i" ignored"}'` 回填
`docs/L1-overview.md` 测试计数行。并复跑 mock e2e（`cargo build -p upstream --features testkit --bin mock-stdio &&
MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored`，2 passed），并
`cd crates/dashboard/ui && npm run build && cd ../../.. && git status --short crates/dashboard/ui/dist`（应为空）。

- [ ] **Step 4: Commit**

```bash
git add docs/L1-overview.md docs/L2-components/dashboard.md
git commit -m "docs: sync L1/L2 for /api/about + About view; backfill test count

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 完成判据（DoD）

- `GET /api/about` 返回版本/git SHA/构建时间 + 非敏感生效配置/限额；**绝不含**密钥/token/env 名/值（类型层面 + 单测断言）。
- 侧栏新增 About 项，About 页分组展示 version/retrieval/dashboard/audit/server/upstreams；加载/失败态正确。
- 端点 12→13；四道门禁 + `assets::` + mock e2e 全绿；L1–L4 同步；dist 可复现、构建 0 警告。
- subagent-driven：每 task spec+质量双审查；最后整分支 audit → 由用户决定合并。
