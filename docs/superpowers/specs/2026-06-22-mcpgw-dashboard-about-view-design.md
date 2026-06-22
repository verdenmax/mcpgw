# Dashboard About / Settings 视图设计

> 状态：已设计待评审 · 日期 2026-06-22 · 子系统 A（只读 dashboard）增量

## 目标

给只读 dashboard 加一个 **About / Settings** 页：展示**生效的非敏感配置/限额** + **版本/构建信息**，
让运维一眼回答"现在跑的是哪个版本/哪个 commit、检索策略是什么、各项限额多少、审计/鉴权开没开"。
**绝不暴露任何密钥/token/env 值/env 变量名**。

## 背景与现状（已核实）

- `AppState`（`crates/dashboard/src/api.rs`）当前字段：`gateway/metrics/discovery/calls/upstreams/strategy/
  audit_path/discovery_path/started_at`——**不含** `call_buffer`/`payload_max_bytes`/`trace_buffer`/
  `trace_queries`/server 配置/`top_k`。故新增**单个** `about: AboutInfo` 字段、在 `main.rs` 从完整 `Config`
  组装最干净（不逐项膨胀 AppState）。
- `Config`：`retrieval`/`upstreams: Vec<UpstreamConfig>`/`server: ServerConfig`/`audit`/`dashboard`。
- `main.rs` 已有 `transport_str(&UpstreamTransport) -> String`（`Stdio→"stdio"`、`Http→"http"`）可复用。
- dashboard 路由现有 12 个 `/api/*`（含 `/api/activity`）。前端 SPA：Svelte 5 + Vite，rust-embed 内嵌 dist；
  中央 `refresh` 控制器；禁 `{@html}`（`assets::no_svelte_component_uses_raw_html`）。

## 非目标（YAGNI）

- 不做任何写/管理操作（保持只读不变量）。
- 不在 About 重复 Overview 的实时指标（uptime/连接数等）；About 是**静态配置/版本**视图，不轮询。
- 不展示密钥/token/env 名/env 值/上游 url 的认证引用。

---

## 后端设计

### 端点

`GET /api/about` → `Json(state.about.clone())`（dashboard 路由，端点数 12 → 13）。`AboutInfo` 在启动时组装一次、
运行期不变，端点零计算。

### 类型 `AboutInfo`（`crates/dashboard/src/about.rs`，全 `#[derive(Serialize, Clone)]`）

```rust
pub struct AboutInfo {
    pub version: VersionInfo,
    pub retrieval: RetrievalInfo,
    pub dashboard: DashboardInfo,
    pub audit: AuditInfo,
    pub server: ServerInfo,
    pub upstreams: Vec<UpstreamConfigInfo>,
}
pub struct VersionInfo { pub version: String, pub git_sha: String, pub build_time: String }
pub struct RetrievalInfo { pub strategy: String, pub top_k: usize }
pub struct DashboardInfo {
    pub call_buffer: usize, pub payload_max_bytes: usize,
    pub trace_queries: bool, pub trace_buffer: usize, pub trace_path: Option<String>,
}
pub struct AuditInfo { pub enabled: bool, pub path: Option<String> }
pub struct ServerInfo {
    pub stdio: bool, pub http_enabled: bool,
    pub http_bind: Option<String>, pub http_path: Option<String>, pub http_auth: bool,
}
pub struct UpstreamConfigInfo { pub name: String, pub transport: String, pub call_timeout_ms: u64 }
```

响应示例：

```jsonc
{
  "version":   { "version": "0.1.0", "git_sha": "3732ec2", "build_time": "2026-06-22T17:00:00Z" },
  "retrieval": { "strategy": "bm25", "top_k": 10 },
  "dashboard": { "call_buffer": 2000, "payload_max_bytes": 16384,
                 "trace_queries": true, "trace_buffer": 500, "trace_path": "mcpgw-discovery.jsonl" },
  "audit":     { "enabled": false, "path": null },
  "server":    { "stdio": false, "http_enabled": true,
                 "http_bind": "127.0.0.1:8970", "http_path": "/mcp", "http_auth": false },
  "upstreams": [ { "name": "mock", "transport": "stdio", "call_timeout_ms": 30000 } ]
}
```

### 组装（`crates/dashboard/src/about.rs` 的纯函数 + `main.rs` 接线）

- `pub fn from_config(cfg: &config::Config, version: VersionInfo) -> AboutInfo`（纯函数，便于单测）：
  - `retrieval`: `cfg.retrieval.strategy.clone()`、`cfg.retrieval.top_k`。
  - `dashboard`: `cfg.dashboard.{call_buffer, payload_max_bytes, trace_queries, trace_buffer}`、
    `trace_path: cfg.dashboard.trace_path.clone()`。
  - `audit`: `enabled: cfg.audit.enabled`、`path: cfg.audit.enabled.then(|| cfg.audit.path.clone())`
    （未启用则 `None`）。
  - `server`: `stdio: cfg.server.stdio`；`http_*` 从 `cfg.server.http`（`Some` 且 `enabled` 时填 bind/path、
    `http_auth = !http.api_keys.is_empty()`；否则 `http_enabled=false`、bind/path `None`、`http_auth=false`）。
  - `upstreams`: `cfg.upstreams.iter().map(|u| UpstreamConfigInfo { name: u.name.clone(),
    transport: transport_str(&u.transport), call_timeout_ms: u.call_timeout_ms })`。
    （`transport_str` 当前在 `main.rs`；组装在 `main.rs` 完成，或把 transport 标签内联映射——见实现备注。）
- `main.rs`：读版本/构建 env 组 `VersionInfo`，`AboutInfo::from_config(&cfg, ver)`，存进 `AppState.about`。

> **职责划分**：`from_config` 的上游映射依赖 transport→标签。为让 `from_config` 纯且自包含，
> `about.rs` 内置一个私有 `transport_label(&config::UpstreamTransport) -> &'static str`（与 main 的
> `transport_str` 同义；二者皆 trivial，重复可接受），避免 dashboard 反向依赖 main。

### 版本 / 构建：`crates/mcpgw/build.rs`

```rust
use std::process::Command;
fn main() {
    let sha = Command::new("git").args(["rev-parse", "--short", "HEAD"]).output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MCPGW_GIT_SHA={sha}");
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=MCPGW_BUILD_TIME={ts}"); // epoch 秒，前端格式化
}
```

`main.rs`：`VersionInfo { version: env!("CARGO_PKG_VERSION").into(),
git_sha: env!("MCPGW_GIT_SHA").into(), build_time: env!("MCPGW_BUILD_TIME").into() }`。

> **构建语义**：build.rs 不强制每次重跑（无 `rerun-if-changed` 把戏），故 `git_sha`/`build_time` 反映**最近一次
> 重建** mcpgw 的状态（"足够好"）；非 git 仓库/无 git → `"unknown"`/`0`，优雅降级。`build_time` 为 epoch 秒、
> 前端转本地时间显示。

### 隐私边界（关键不变量）

`AboutInfo` 及其嵌套类型**字段集里根本不含**任何密钥/token/env 名/env 值/上游认证引用：
- `server.http_auth` 仅 `bool`（`api_keys` 非空 = true），**不含** `ApiKeyConfig.name`/`.env`。
- `upstreams` 仅 `name`/`transport`/`call_timeout_ms`，**不含** http 上游的 `url`/`bearer_env`/`headers`。
- `trace_path`/`audit.path` 为运维侧文件路径（非密钥）——展示。
- 单测断言：用含 `[[server.http.api_key]]`（`name="k" env="SECRET_KEY"`）+ http 上游（`bearer_env="TOK"`）的
  Config 组 `AboutInfo`，序列化 JSON **不含** `"SECRET_KEY"`/`"TOK"`/`"k"`/`bearer`/`api_key`，但 `http_auth==true`。

---

## 前端设计

- **`About.svelte`**（新）：`onMount` 拉一次 `/api/about`（配置不变、**不轮询**）。渲染分组（沿用 `table.kv`/`.cards`/
  `.empty` 既有样式）：
  - **Version**：version / git_sha（mono）/ build_time（`new Date(secs*1000).toLocaleString()`）。
  - **Retrieval**：strategy / top_k。
  - **Dashboard**：call_buffer / payload_max_bytes / trace_queries / trace_buffer / trace_path(`?? "—"`)。
  - **Audit**：enabled（badge ok/—）/ path(`?? "—"`)。
  - **Server**：stdio / http_enabled / http_bind / http_path / http_auth（"enabled"/"disabled" badge）。
  - **Upstreams**：小表 name / transport / call_timeout_ms。
  - 失败 → `.error role=alert`；加载中 → `.skeleton`。
- **`Nav.svelte`**：导航末尾加 `["about","About","info"]`。
- **`Icon.svelte`**：加 `info` 图标（`<circle r=9/> + i`：`<line>`/`<circle>` 画 "i"）。
- **`App.svelte` + `router`**：加 `{:else if route.view === "about"}<About />`。
- 全只读、无 `{@html}`、chips/links 为原生元素。

---

## 测试

- **后端单测**（`about.rs`）：
  - `from_config` 映射正确（strategy/top_k/dashboard 限额/audit enabled→path/server http 字段/upstreams）。
  - audit 未启用 → `audit.path == None`；http 段缺失 → `http_enabled=false`、bind/path `None`、`http_auth=false`。
  - http 有 `api_key` → `http_auth=true`。
  - **隐私**：序列化 JSON 不含上述任何 env 名/值/key 名（见隐私边界）。
- **e2e**（`crates/mcpgw/tests/dashboard.rs`，mock 上游）：`GET /api/about` 断言 `version.version` 非空、
  `server.http_auth==false`、`upstreams` 含 `{name:"mock"}`。
- **前端**：`assets::` 3/3、构建 0 警告、dist 可复现。
- **四道门禁**全绿；记录新计数回填 L1。

## 文档（随码同提交）

- **L1**：端点 12→13；新增 About 视图一句；测试计数回填。
- **L2 `dashboard.md`**：`AboutInfo`/`from_config`/`/api/about`、`AppState.about`。
- **L3 `dashboard.md`**：About 组装（启动一次、运行期不变）+ **隐私边界**（绝不含密钥/env 值）。
- **L4 `dashboard.md`**：`/api/about` 端点 + `AboutInfo` 各嵌套类型字段。
- **L4 `mcpgw-main.md`**：`build.rs`（git SHA/build time、优雅降级、构建语义）+ `AboutInfo` 组装接线进 `AppState`。

## 里程碑拆分（单一计划）

建议 task 序：
1. 后端 `about.rs`（类型 + `from_config` + 单测含隐私断言）。
2. build.rs + `main.rs` 接线（`VersionInfo` + `AppState.about`）+ `/api/about` 路由 + e2e + L3/L4 文档。
3. 前端 `About.svelte` + Nav/Icon/App/router + 构建。
4. L1/L2 文档 + 四道门禁 + 计数回填。

执行：subagent-driven，每 task spec+质量双审查；最后整分支 audit → 由用户决定合并。

## 完成判据（DoD）

- `GET /api/about` 返回版本/构建 + 非敏感生效配置/限额；**绝不含**密钥/token/env 名/值（类型层面 + 单测断言）。
- 新 About 页在侧栏可达,分组展示上述信息;失败/加载态正确。
- 四道门禁 + assets + e2e 全绿；L1–L4 同步；dist 可复现、构建 0 警告。
