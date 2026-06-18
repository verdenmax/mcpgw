# M3：Upstreams / Tools / Traces 下钻详情页 + 交叉链接 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 M2 的下钻应用上为 Upstreams / Tools / Traces 三区域补齐**详情页**与**交叉链接**：点 upstream 看其工具列表 + 最近调用；点 tool 看 schema + 所属上游 + 最近调用；点 trace 看 query/命中工具+分数（命中可再点进 tool 详情）；Overview 卡片与 CallDetail 的链接都接到真实详情页。三区域形成「列表 → 详情 → 交叉跳转」的闭环。

**Architecture:** 按**功能纵切**拆任务（每个 task 同时落后端端点 + 前端详情页 + 列表行链接，保证每步结束 dashboard 都可用、无破窗）。新增三个只读端点：`GET /api/upstreams/{name}`、`GET /api/tools/{name}`、`GET /api/traces/{id}`；其中 traces 需要给追踪记录分配稳定 id（**完全镜像 M1 的 CallItem/seq 模式**：live 环单调 seq + history 复合 `"h{ts}-{n}"`）。前端新增三个详情视图组件 + 路由分发（`#/upstreams/{name}`、`#/tools/{name}`、`#/traces/{id}`），并把 hash 路由参数做 `decodeURIComponent`。详情页的「最近调用」复用已有 `/api/calls?upstream=&tool=`，无需新端点。

**Tech Stack:** Rust（axum 0.8 路径参数 `{name}`/`{id}`、serde、catalog `ToolDef`、observe `DiscoveryRecord`）、Svelte 5 + Vite（产物 `ui/dist/` 入库、rust-embed 内嵌）。

**关键约束：**
- **不破窗**：每个 task 后端+前端一起改，结束时 `npm run build` + 四门禁均绿、dashboard 完整可用。
- **dist 同步**：任何改 `ui/src/` 的提交必须同时 `npm run build` 并提交 `ui/dist/`。
- **XSS**：只用 Svelte `{表达式}`（自动转义），**禁** `{@html}`（`assets.rs` 的 `no_svelte_component_uses_raw_html` 测试会扫描 `ui/src` 强制）。tool 的 `input_schema` 是 JSON，用 `JSON.stringify(..,null,2)` 渲染进 `<pre>{...}</pre>`（文本插值、自动转义）。
- **trace id 镜像 M1**：DiscoveryRingSink 加 `next_seq`（锁内分配）、`TraceItem{id,...}` owned 包装、history 回放分配 `"h{ts}-{n}"`，与 `CallRingSink`/`replay_audit_calls` 完全同构。
- **复用**：详情页「最近调用」复用 `/api/calls?upstream=&tool=&limit=`；不新增 calls 端点。
- **端点计数**：8 → **11**（+upstreams/{name}、tools/{name}、traces/{id}）。

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/dashboard/src/api.rs` | `UpstreamDetail`/`ToolDetail` 视图 + `upstream_detail`/`tool_detail`/`trace_detail` 纯函数；`traces()` 返回 `TraceItem` | 修改 |
| `crates/dashboard/src/trace.rs` | `DiscoveryRingSink` 加 `next_seq` + `TraceItem` + `recent`→`Vec<TraceItem>` + `get(seq)` | 修改 |
| `crates/dashboard/src/history.rs` | 新增 `replay_discovery_items`（→ `Vec<TraceItem>`，稳定 `"h{ts}-{n}"` id） | 修改 |
| `crates/dashboard/src/lib.rs` | 三个 detail handler + 三条路由 | 修改 |
| `crates/dashboard/ui/src/lib/UpstreamDetail.svelte` / `ToolDetail.svelte` / `TraceDetail.svelte` | 三个详情视图 | 新建 |
| `crates/dashboard/ui/src/lib/{Upstreams,Tools,Traces,Overview,CallDetail}.svelte` | 列表行/卡片/交叉链接接入详情页 | 修改 |
| `crates/dashboard/ui/src/lib/router.svelte.js` | 路由参数 `decodeURIComponent` | 修改 |
| `crates/dashboard/ui/src/App.svelte` | 三区域 detail 路由分发 | 修改 |
| `crates/dashboard/ui/dist/**` | 构建产物 | 重新生成 |
| `docs/L1-overview.md` / `docs/L2-components/dashboard.md` / `docs/L3-details/dashboard.md` / `docs/L4-api/dashboard.md` | 分层文档同步 | 修改 |

---

## Task 1：Upstreams 下钻（`/api/upstreams/{name}` + 详情页 + 列表/Overview 链接）

**Files:**
- Modify: `crates/dashboard/src/api.rs`（`UpstreamDetail` + `upstream_detail`），`crates/dashboard/src/lib.rs`（handler + 路由）
- Create: `crates/dashboard/ui/src/lib/UpstreamDetail.svelte`
- Modify: `crates/dashboard/ui/src/lib/{Upstreams,Overview}.svelte`、`router.svelte.js`、`App.svelte`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

### 后端

- [ ] **Step 1: 写失败测试（api.rs）**

在 `crates/dashboard/src/api.rs` 测试模块追加（复用 `seeded_state()`）：
```rust
    #[tokio::test]
    async fn upstream_detail_unknown_is_none() {
        let st = seeded_state().await;
        assert!(upstream_detail(&st, "nope").is_none());
    }

    #[tokio::test]
    async fn upstream_detail_returns_view_and_tools() {
        // seeded_state configures one upstream "github" (transport stdio) with an empty snapshot.
        let st = seeded_state().await;
        let d = upstream_detail(&st, "github").expect("configured upstream resolves");
        assert_eq!(d.name, "github");
        assert_eq!(d.transport, "stdio");
        assert_eq!(d.status, "unknown"); // no rebuild summary in seeded_state
        assert!(d.tools.is_empty(), "empty catalog -> no tools");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard api::tests::upstream_detail`
Expected: 编译错误 `cannot find function upstream_detail` / `UpstreamDetail`。

- [ ] **Step 3: 实现（api.rs）**

新增视图类型（放在 `UpstreamView` 附近）：
```rust
#[derive(Serialize)]
pub struct UpstreamDetail {
    pub name: String,
    pub transport: String,
    pub status: &'static str,
    pub reason: Option<String>,
    pub tools_count: usize,
    pub calls: u64,
    pub errors: u64,
    pub tools: Vec<ToolView>,
}
```
新增纯函数（放在 `upstreams` 之后；复用其状态判定逻辑）：
```rust
/// Single-upstream detail: its `UpstreamView` fields + the list of tools it currently exposes.
/// `None` if `name` isn't a configured upstream.
pub fn upstream_detail(state: &AppState, name: &str) -> Option<UpstreamDetail> {
    let info = state.upstreams.iter().find(|u| u.name == name)?;
    let snap = state.gateway.snapshot();
    let summary = state.gateway.last_summary();
    let m = state.metrics.snapshot();
    let (status, reason) = match &summary {
        None => ("unknown", None),
        Some(s) => {
            if s.ingested.iter().any(|n| n == &info.name) {
                ("connected", None)
            } else if let Some((_, why)) = s.skipped.iter().find(|(n, _)| n == &info.name) {
                ("skipped", Some(why.clone()))
            } else {
                ("unknown", None)
            }
        }
    };
    let tools: Vec<ToolView> = snap
        .catalog()
        .iter()
        .filter(|t| t.server == info.name)
        .map(|t| ToolView {
            name: t.qualified_name(),
            description: t.description.clone(),
        })
        .collect();
    let um = m.per_upstream.iter().find(|u| u.upstream == info.name);
    Some(UpstreamDetail {
        name: info.name.clone(),
        transport: info.transport.clone(),
        status,
        reason,
        tools_count: tools.len(),
        calls: um.map(|u| u.calls).unwrap_or(0),
        errors: um.map(|u| u.errors).unwrap_or(0),
        tools,
    })
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dashboard api::tests::upstream_detail`
Expected: PASS（2 测试）。

- [ ] **Step 5: handler + 路由（lib.rs）**

`crates/dashboard/src/lib.rs` 新增 handler（`h_call_detail` 附近）：
```rust
async fn h_upstream_detail(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> axum::response::Response {
    match api::upstream_detail(&s, &name) {
        Some(d) => Json(d).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
```
路由（在 `/api/upstreams` 之后）：
```rust
        .route("/api/upstreams/{name}", get(h_upstream_detail))
```

- [ ] **Step 6: 后端验证**

Run: `cargo test -p dashboard && cargo clippy -p dashboard --all-targets -- -D warnings && cargo fmt -p dashboard --check`
Expected: 全过、无 warning、无 diff。

### 前端

- [ ] **Step 7: 路由参数解码（router.svelte.js）**

`crates/dashboard/ui/src/lib/router.svelte.js` 的 `parse()`：把每段做 `decodeURIComponent`，使带特殊字符的 name 正确还原。改 `parse`：
```js
function parse() {
  const raw = window.location.hash.replace(/^#\/?/, "");
  const parts = raw.split("/").filter(Boolean).map((p) => {
    try { return decodeURIComponent(p); } catch (_) { return p; }
  });
  return { view: parts[0] || "overview", params: parts.slice(1) };
}
```

- [ ] **Step 8: `UpstreamDetail.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let { name } = $props();
  let d = $state(null);
  let calls = $state([]);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      const r = await fetch(`/api/upstreams/${encodeURIComponent(name)}`);
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/upstreams/${name} -> ${r.status}`);
      d = await r.json(); notFound = false;
      const c = await getJSON(`/api/calls?source=live&upstream=${encodeURIComponent(name)}&limit=20`);
      calls = c.items ?? []; error = null;
    } catch (e) { error = String(e); }
  }
  $effect(() => { name; load(); });
  function when(ms) { return new Date(ms).toLocaleString(); }
</script>

<p><a href="#/upstreams">‹ back to Upstreams</a></p>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">upstream not found</p>
{:else if d}
  <h2>{d.name}</h2>
  <div class="cards">
    <div class="card"><div class="label">transport</div><div class="v">{d.transport}</div></div>
    <div class="card"><div class="label">status</div><div class="v"><span class="badge {d.status}">{d.status}</span></div></div>
    <div class="card"><div class="label">tools</div><div class="v">{d.tools_count}</div></div>
    <div class="card"><div class="label">calls</div><div class="v">{d.calls}</div></div>
    <div class="card"><div class="label">errors</div><div class="v">{d.errors}</div></div>
  </div>
  {#if d.reason}<p class="muted">reason: {d.reason}</p>{/if}

  <h3>Tools</h3>
  <table>
    <thead><tr><th>name</th><th>description</th></tr></thead>
    <tbody>
      {#each d.tools as t}
        <tr class="row-link" onclick={() => (location.hash = `#/tools/${encodeURIComponent(t.name)}`)}>
          <td>{t.name}</td><td>{t.description}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  <h3>Recent calls</h3>
  <table>
    <thead><tr><th>time</th><th>meta</th><th>target</th><th>outcome</th><th>ms</th></tr></thead>
    <tbody>
      {#each calls as c}
        <tr class="row-link" onclick={() => (location.hash = `#/calls/${c.id}`)}>
          <td>{when(c.ts_unix_ms)}</td><td>{c.meta_tool}</td><td>{c.target_tool ?? "—"}</td><td>{c.outcome}</td><td>{c.latency_ms}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
```

- [ ] **Step 9: 列表行 + Overview 卡片接链接**

`Upstreams.svelte`：表格行加可点跳详情（每行 `<tr class="row-link" onclick={() => (location.hash = `#/upstreams/${encodeURIComponent(u.name)}`)}>`）。
`Overview.svelte`：把 `upstreams` 卡片与 `calls` 卡片包成可点链接 —— upstreams 卡外层加 `class="card row-link"` + `onclick={() => (location.hash = "#/upstreams")}`；calls 卡 `onclick={() => (location.hash = "#/calls")}`。

- [ ] **Step 10: `App.svelte` 路由分发**

import `UpstreamDetail`，并在 `upstreams` 分支前加 detail 分支：
```svelte
    {:else if route.view === "upstreams" && route.params.length > 0}
      <UpstreamDetail name={route.params[0]} />
    {:else if route.view === "upstreams"}
      <Upstreams />
```

- [ ] **Step 11: 构建 + 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd ../../.. && cargo test -p dashboard assets::`
Expected: 构建成功（a11y 警告非致命）；`assets::` 3/3。
```bash
git add crates/dashboard/src/api.rs crates/dashboard/src/lib.rs crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard): Upstreams drill-down — /api/upstreams/{name} + detail page + links

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：Tools 下钻（`/api/tools/{name}` + schema 详情页 + Tools/Overview/CallDetail 链接）

**Files:**
- Modify: `crates/dashboard/src/api.rs`（`ToolDetail` + `tool_detail`），`lib.rs`（handler + 路由）
- Create: `crates/dashboard/ui/src/lib/ToolDetail.svelte`
- Modify: `crates/dashboard/ui/src/lib/{Tools,Overview,CallDetail}.svelte`、`App.svelte`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

### 后端

- [ ] **Step 1: 写失败测试（api.rs）**

```rust
    #[tokio::test]
    async fn tool_detail_unknown_is_none() {
        let st = seeded_state().await;
        assert!(tool_detail(&st, "nope__missing").is_none());
    }
```
> 注：`seeded_state()` 的 catalog 为空，故只能测「未命中→None」。命中路径由 Task 5 的 ignored e2e（真实上游 + 真实 catalog）覆盖。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard api::tests::tool_detail`
Expected: 编译错误 `cannot find function tool_detail` / `ToolDetail`。

- [ ] **Step 3: 实现（api.rs）**

视图类型：
```rust
#[derive(Serialize)]
pub struct ToolDetail {
    pub name: String,          // qualified_name {server}__{tool}
    pub server: String,        // owning upstream
    pub description: String,
    pub input_schema: serde_json::Value,
}
```
纯函数（catalog 按 qualified_name 取，复用既有 `Catalog::get`）：
```rust
/// Single-tool detail from the catalog (keyed by qualified name `{server}__{tool}`). `None` if absent.
pub fn tool_detail(state: &AppState, name: &str) -> Option<ToolDetail> {
    let snap = state.gateway.snapshot();
    let def = snap.catalog().get(name)?;
    Some(ToolDetail {
        name: def.qualified_name(),
        server: def.server.clone(),
        description: def.description.clone(),
        input_schema: def.input_schema.clone(),
    })
}
```
> `snap` 持有快照 `Arc`；`def` 借自其中的 `Catalog`，在 `snap` drop 前 clone 出所需字段即可（上面已 clone）。`GatewaySnapshot::catalog()` 是只读访问器，`Catalog::get(qualified_name) -> Option<&ToolDef>`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dashboard api::tests::tool_detail`
Expected: PASS。

- [ ] **Step 5: handler + 路由（lib.rs）**

```rust
async fn h_tool_detail(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> axum::response::Response {
    match api::tool_detail(&s, &name) {
        Some(d) => Json(d).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
```
路由（在 `/api/tools` 之后）：
```rust
        .route("/api/tools/{name}", get(h_tool_detail))
```

- [ ] **Step 6: 后端验证**

Run: `cargo test -p dashboard && cargo clippy -p dashboard --all-targets -- -D warnings && cargo fmt -p dashboard --check`
Expected: 全过、无 warning、无 diff。

### 前端

- [ ] **Step 7: `ToolDetail.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let { name } = $props();
  let d = $state(null);
  let calls = $state([]);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      const r = await fetch(`/api/tools/${encodeURIComponent(name)}`);
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/tools/${name} -> ${r.status}`);
      d = await r.json(); notFound = false;
      const c = await getJSON(`/api/calls?source=live&tool=${encodeURIComponent(name)}&limit=20`);
      calls = c.items ?? []; error = null;
    } catch (e) { error = String(e); }
  }
  $effect(() => { name; load(); });
  function when(ms) { return new Date(ms).toLocaleString(); }
  function schema(v) { try { return JSON.stringify(v, null, 2); } catch (_) { return String(v); } }
</script>

<p><a href="#/tools">‹ back to Tools</a></p>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">tool not found</p>
{:else if d}
  <h2>{d.name}</h2>
  <p>upstream: <a href={`#/upstreams/${encodeURIComponent(d.server)}`}>{d.server}</a></p>
  <p>{d.description}</p>
  <h3>Input schema</h3>
  <pre class="schema">{schema(d.input_schema)}</pre>

  <h3>Recent calls</h3>
  <table>
    <thead><tr><th>time</th><th>meta</th><th>outcome</th><th>ms</th></tr></thead>
    <tbody>
      {#each calls as c}
        <tr class="row-link" onclick={() => (location.hash = `#/calls/${c.id}`)}>
          <td>{when(c.ts_unix_ms)}</td><td>{c.meta_tool}</td><td>{c.outcome}</td><td>{c.latency_ms}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
```
并给 `crates/dashboard/ui/src/app.css` 追加：
```css
.schema { background:var(--panel); border:1px solid var(--border); border-radius:6px; padding:10px; overflow:auto; max-height:340px; white-space:pre; }
h3 { margin-top:20px; }
```

- [ ] **Step 8: 列表行 + Overview 卡片 + CallDetail 接链接**

`Tools.svelte`：表格行加可点（`<tr class="row-link" onclick={() => (location.hash = `#/tools/${encodeURIComponent(t.name)}`)}>`）。
`Overview.svelte`：tools 卡片包成 `class="card row-link"` + `onclick={() => (location.hash = "#/tools")}`。
`CallDetail.svelte`：把 target_tool / upstream 的链接从列表页改为详情页：
- `target_tool`：`<a href={`#/tools/${encodeURIComponent(item.target_tool)}`}>{item.target_tool}</a>`
- `upstream`：`<a href={`#/upstreams/${encodeURIComponent(item.upstream)}`}>{item.upstream}</a>`

- [ ] **Step 9: `App.svelte` 路由分发**

import `ToolDetail`，在 `tools` 分支前加 detail 分支：
```svelte
    {:else if route.view === "tools" && route.params.length > 0}
      <ToolDetail name={route.params[0]} />
    {:else if route.view === "tools"}
      <Tools />
```

- [ ] **Step 10: 构建 + 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd ../../.. && cargo test -p dashboard assets::`
Expected: 构建成功；`assets::` 3/3（含 `{@html}` 守护——schema 用 `<pre>{...}</pre>` 文本插值，无 `{@html}`）。
```bash
git add crates/dashboard/src/api.rs crates/dashboard/src/lib.rs crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard): Tools drill-down — /api/tools/{name} + schema detail + cross-links

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：Traces 下钻（trace id + `/api/traces/{id}` + 详情页 + 命中交叉链接）

**Files:**
- Modify: `crates/dashboard/src/trace.rs`（`TraceItem` + `next_seq` + `recent`→`Vec<TraceItem>` + `get`），`crates/dashboard/src/history.rs`（`replay_discovery`→`replay_discovery_items`），`api.rs`（`traces` 返回 `TraceItem`、`trace_detail`、`TRACE_HISTORY_SCAN`），`lib.rs`（导出 + handler + 路由）
- Create: `crates/dashboard/ui/src/lib/TraceDetail.svelte`
- Modify: `crates/dashboard/ui/src/lib/Traces.svelte`、`App.svelte`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

> **镜像 M1 calls**：给追踪记录分配稳定 id（live=环内单调 `seq`、history=`"h{ts}-{n}"`），detail 与 list 共用同一扫描窗口 `TRACE_HISTORY_SCAN`，故 list 分配的 id 在 detail 稳定复现。`DiscoveryRecord` 本身派生 `Deserialize`（不像 `CallRecord`），故 history 回放可直接反序列化、无需 owned 镜像。JSONL 文件格式不变（id 在读取/回放时分配）。

### 后端

- [ ] **Step 1: 写失败测试（trace.rs）**

在 `crates/dashboard/src/trace.rs` 测试模块追加（现有 helper `rec(q)` 造 `DiscoveryRecord`）：
```rust
    #[test]
    fn ring_assigns_seq_ids_newest_first_and_get_resolves() {
        let (sink, _w) = DiscoveryRingSink::spawn(10, None).unwrap();
        sink.record(&rec("a"));
        sink.record(&rec("b"));
        let items = sink.recent(10);
        // newest-first: ids "1" then "0"
        assert_eq!(items[0].id, "1");
        assert_eq!(items[0].query, "b");
        assert_eq!(items[1].id, "0");
        let got = sink.get(0).expect("seq 0 present");
        assert_eq!(got.query, "a");
        assert!(sink.get(999).is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dashboard trace::`
Expected: 编译/断言失败（`recent` 仍返回 `DiscoveryRecord`、无 `id`/`get`）。

- [ ] **Step 3: 实现（trace.rs）**

在 import 区确保有 `use std::sync::atomic::{AtomicU64, Ordering};`（文件已用 `AtomicU64` 于 `dropped`，复用）。新增 owned 项类型与内部条目：
```rust
use observe::DiscoveryHit;

/// One discovery trace as exposed by the API: a stable id (live = decimal ring seq; history =
/// `"h{ts}-{n}"`) + the trace fields. Mirrors `calls::CallItem`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TraceItem {
    pub id: String,
    pub ts_unix_ms: u64,
    pub query: String,
    pub top_k: usize,
    pub results: Vec<DiscoveryHit>,
    pub latency_ms: u64,
}

struct StoredTrace {
    seq: u64,
    record: DiscoveryRecord,
}

impl StoredTrace {
    fn to_item(&self) -> TraceItem {
        let r = &self.record;
        TraceItem {
            id: self.seq.to_string(),
            ts_unix_ms: r.ts_unix_ms,
            query: r.query.clone(),
            top_k: r.top_k,
            results: r.results.clone(),
            latency_ms: r.latency_ms,
        }
    }
}
```
把 `ring: Mutex<VecDeque<DiscoveryRecord>>` 改为 `ring: Mutex<VecDeque<StoredTrace>>`，结构体加 `next_seq: AtomicU64`，`spawn` 里初始化 `next_seq: AtomicU64::new(0)`。改 `recent` 与新增 `get`：
```rust
    /// Most recent traces, newest first, capped at `limit` (each with its live id = ring seq).
    pub fn recent(&self, limit: usize) -> Vec<TraceItem> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().rev().take(limit).map(|s| s.to_item()).collect()
    }

    /// Resolve a live id (decimal seq) to its trace item, or `None` if evicted/never existed.
    pub fn get(&self, seq: u64) -> Option<TraceItem> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().find(|s| s.seq == seq).map(|s| s.to_item())
    }
```
改 `DiscoverySink::record`（seq 锁内分配；writer 仍序列化原始 `DiscoveryRecord`）：
```rust
    fn record(&self, rec: &DiscoveryRecord) {
        {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
            if ring.len() == self.cap {
                ring.pop_front();
            }
            ring.push_back(StoredTrace { seq, record: rec.clone() });
        }
        if let Some(tx) = &self.tx {
            if let Ok(line) = serde_json::to_string(rec) {
                if let Err(TrySendError::Full(_)) = tx.try_send(line) {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
```
更新该文件里既有的 `ring_caps_and_returns_newest_first` / `recent_respects_limit` 测试：`recent` 现返回 `TraceItem`，断言改为读 `.query`（而非 `DiscoveryRecord`）。`lib.rs` 导出 `TraceItem`：在 `pub use trace::{...}` 里加 `TraceItem`。

- [ ] **Step 4: 实现（history.rs）—— `replay_discovery` → `replay_discovery_items`**

把现有 `replay_discovery`（返回 `Vec<DiscoveryRecord>`）替换为返回 `Vec<TraceItem>` 并分配稳定 id：
```rust
use crate::trace::TraceItem;

/// Replay the discovery JSONL into `TraceItem`s, newest-first, scanning at most the last `limit`
/// lines; bad lines skipped. Each item gets a stable id `"h{ts}-{n}"` (n counts same-ts in file
/// order). `DiscoveryRecord` derives `Deserialize`, so no owned-mirror is needed. Bool = readable.
pub fn replay_discovery_items(path: &Path, limit: usize) -> (Vec<TraceItem>, bool) {
    let Some(lines) = tail_lines(path, limit) else {
        return (Vec::new(), false);
    };
    let mut ts_counts: std::collections::BTreeMap<u64, u32> = std::collections::BTreeMap::new();
    let mut items: Vec<TraceItem> = Vec::new();
    for line in &lines {
        if let Ok(r) = serde_json::from_str::<observe::DiscoveryRecord>(line) {
            let n = ts_counts.entry(r.ts_unix_ms).or_insert(0);
            let id = format!("h{}-{}", r.ts_unix_ms, *n);
            *n += 1;
            items.push(TraceItem {
                id,
                ts_unix_ms: r.ts_unix_ms,
                query: r.query,
                top_k: r.top_k,
                results: r.results,
                latency_ms: r.latency_ms,
            });
        }
    }
    items.reverse();
    (items, true)
}
```
更新 `lib.rs` 的 `pub use history::{...}`：把 `replay_discovery` 换成 `replay_discovery_items`。更新 history.rs 里既有的 `replay_discovery_*` 测试为新名 + 断言 `TraceItem`（newest-first、坏行跳过、id 形如 `"h{ts}-{n}"`）。

- [ ] **Step 5: 实现（api.rs）—— traces 返回 TraceItem + trace_detail**

`TracesResponse.traces` 类型 `Vec<observe::DiscoveryRecord>` → `Vec<crate::trace::TraceItem>`。`use` 区把 `replay_discovery` 换成 `replay_discovery_items`。新增常量 + 改 `traces` + 新增 `trace_detail`：
```rust
/// Discovery lines scanned for BOTH the traces list (source=history) and single-id resolution, so a
/// history id assigned by the list always resolves identically in detail (mirrors CALL_HISTORY_SCAN).
pub const TRACE_HISTORY_SCAN: usize = 50_000;
```
```rust
pub fn traces(state: &AppState, limit: usize, source: &str) -> TracesResponse {
    if source == "history" {
        match &state.discovery_path {
            Some(p) => {
                let (mut traces, ok) = replay_discovery_items(p, TRACE_HISTORY_SCAN);
                traces.truncate(limit);
                TracesResponse { source: "history".into(), history_unavailable: !ok, traces }
            }
            None => TracesResponse { source: "history".into(), history_unavailable: true, traces: Vec::new() },
        }
    } else {
        let traces = state.discovery.as_ref().map(|d| d.recent(limit)).unwrap_or_default();
        TracesResponse { source: "live".into(), history_unavailable: false, traces }
    }
}

/// Resolve one trace id: `h...` -> history (re-scan TRACE_HISTORY_SCAN + find), else decimal seq ->
/// live ring. `None` if not found / source unavailable.
pub fn trace_detail(state: &AppState, id: &str) -> Option<crate::trace::TraceItem> {
    if id.starts_with('h') {
        let p = state.discovery_path.as_ref()?;
        let (items, ok) = replay_discovery_items(p, TRACE_HISTORY_SCAN);
        if !ok { return None; }
        items.into_iter().find(|t| t.id == id)
    } else {
        let seq: u64 = id.parse().ok()?;
        state.discovery.as_ref()?.get(seq)
    }
}
```
更新 api.rs 既有的 `traces_history_unavailable_without_path` 测试（仍成立：history_unavailable=true、traces 空）。新增：
```rust
    #[tokio::test]
    async fn trace_detail_live_by_seq_and_404() {
        use observe::{DiscoverySink};
        let (ring, _w) = crate::trace::DiscoveryRingSink::spawn(10, None).unwrap();
        ring.record(&observe::DiscoveryRecord { ts_unix_ms: 5, query: "weather".into(), top_k: 1, results: vec![], latency_ms: 2 });
        let st = AppState { discovery: Some(std::sync::Arc::new(ring)), ..seeded_state().await };
        let t = trace_detail(&st, "0").expect("seq 0 present");
        assert_eq!(t.query, "weather");
        assert!(trace_detail(&st, "999").is_none());
        assert!(trace_detail(&st, "not-a-number").is_none());
    }
```

- [ ] **Step 6: handler + 路由（lib.rs）**

```rust
async fn h_trace_detail(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    // History path reads a JSONL file off the blocking pool (mirrors h_call_detail).
    if id.starts_with('h') {
        let detail = tokio::task::spawn_blocking(move || api::trace_detail(&s, &id))
            .await
            .expect("trace detail replay task");
        match detail { Some(t) => Json(t).into_response(), None => StatusCode::NOT_FOUND.into_response() }
    } else {
        match api::trace_detail(&s, &id) {
            Some(t) => Json(t).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}
```
路由（在 `/api/traces` 之后）：`.route("/api/traces/{id}", get(h_trace_detail))`。

- [ ] **Step 7: 后端全验证**

Run: `cargo test -p dashboard && cargo clippy -p dashboard --all-targets -- -D warnings && cargo fmt -p dashboard --check`
Expected: 全过、无 warning、无 diff（确认 `replay_discovery` 旧名无残留引用）。

### 前端

- [ ] **Step 8: `TraceDetail.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  let { id } = $props();
  let t = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      const r = await fetch(`/api/traces/${encodeURIComponent(id)}`);
      if (r.status === 404) { notFound = true; t = null; return; }
      if (!r.ok) throw new Error(`/api/traces/${id} -> ${r.status}`);
      t = await r.json(); notFound = false; error = null;
    } catch (e) { error = String(e); }
  }
  $effect(() => { id; load(); });
  function when(ms) { return new Date(ms).toLocaleString(); }
</script>

<p><a href="#/traces">‹ back to Traces</a></p>
<h2>Trace detail</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">trace not found (it may have aged out of the live ring)</p>
{:else if t}
  <table>
    <tbody>
      <tr><th>id</th><td>{t.id}</td></tr>
      <tr><th>time</th><td>{when(t.ts_unix_ms)}</td></tr>
      <tr><th>query</th><td>{t.query}</td></tr>
      <tr><th>top_k</th><td>{t.top_k}</td></tr>
      <tr><th>latency_ms</th><td>{t.latency_ms}</td></tr>
    </tbody>
  </table>
  <h3>Hits ({t.results.length})</h3>
  <table>
    <thead><tr><th>tool</th><th>score</th></tr></thead>
    <tbody>
      {#each t.results as h}
        <tr class="row-link" onclick={() => (location.hash = `#/tools/${encodeURIComponent(h.name)}`)}>
          <td>{h.name}</td><td>{h.score.toFixed(3)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
```

- [ ] **Step 9: `Traces.svelte` 卡片可点 + `App.svelte` 路由**

`Traces.svelte`：每条 trace 卡片加可点跳详情（外层 `class="card trace-card row-link"` + `onclick={() => (location.hash = `#/traces/${t.id}`)}`）；卡内仍展示 query + 命中 chip（保持现状，命中在详情页才作 tool 链接，避免列表里嵌套点击）。
`App.svelte`：import `TraceDetail`，在 `traces` 分支前加 detail：
```svelte
    {:else if route.view === "traces" && route.params.length > 0}
      <TraceDetail id={route.params[0]} />
    {:else if route.view === "traces"}
      <Traces />
```

- [ ] **Step 10: 构建 + 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd ../../.. && cargo test -p dashboard assets::`
Expected: 构建成功；`assets::` 3/3。
```bash
git add crates/dashboard/src/trace.rs crates/dashboard/src/history.rs crates/dashboard/src/api.rs crates/dashboard/src/lib.rs crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard): Traces drill-down — trace ids + /api/traces/{id} + detail + hit links

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：e2e 断言 + 分层文档同步 + 四道门禁

**Files:**
- Modify: `crates/mcpgw/tests/dashboard.rs`（ignored e2e 加详情端点断言）
- Modify: `docs/L1-overview.md` / `docs/L2-components/dashboard.md` / `docs/L3-details/dashboard.md` / `docs/L4-api/dashboard.md` / `docs/README.md`

### e2e（HTTP 层覆盖新端点）

- [ ] **Step 1: 扩 ignored e2e**

现有 `dashboard_serves_api_and_captures_a_trace`（`#[ignore]`）已驱动一次 `search_tools`（产生一条 live trace），且其配置无上游（catalog 空）。在 `client.cancel()` 之前追加：
```rust
    // M3: trace detail happy-path (the search above created a live trace with an id).
    let traces: serde_json::Value = http
        .get(format!("{base}/api/traces?source=live&limit=10"))
        .send().await.unwrap().json().await.unwrap();
    let tid = traces["traces"][0]["id"].as_str().expect("a live trace id").to_string();
    let td = http.get(format!("{base}/api/traces/{tid}")).send().await.unwrap();
    assert_eq!(td.status(), 200);
    let tdj: serde_json::Value = td.json().await.unwrap();
    assert_eq!(tdj["query"], "weather forecast");

    // M3: unknown upstream / tool detail -> 404 (this config has no upstreams / empty catalog).
    assert_eq!(http.get(format!("{base}/api/upstreams/nope")).send().await.unwrap().status(), 404);
    assert_eq!(http.get(format!("{base}/api/tools/nope__missing")).send().await.unwrap().status(), 404);
    assert_eq!(http.get(format!("{base}/api/traces/h9-9")).send().await.unwrap().status(), 404);
```
Run: `cargo test -p mcpgw --test dashboard -- --ignored`
Expected: 1 passed.

> upstream/tool 详情的**命中路径**（非空 catalog）由单测的逻辑 + 手动冒烟（带 mock 上游的演示配置）覆盖；本 e2e 配置无上游，故只断言 404 + trace 命中路径。

### 文档

- [ ] **Step 2: L4（`docs/L4-api/dashboard.md`）** —— READ 后：
  1. 开头端点数 `8 个` → `11 个`。
  2. `calls.rs` 思路类比，在相应位置补 `### struct UpstreamDetail` / `### struct ToolDetail`（视图类型）与 `### struct TraceItem`（`trace.rs` 段，含 id 语义 live=seq / history=`"h{ts}-{n}"`）。
  3. `trace.rs` 段：`DiscoveryRingSink` 现内部存 `StoredTrace{seq,record}`、`recent(limit) -> Vec<TraceItem>`（最新优先、带 live id）、新增 `get(seq) -> Option<TraceItem>`；`record` 锁内分配 seq；JSONL writer 仍写原始 `DiscoveryRecord`（id 在读取时分配）。
  4. `history.rs` 段：`replay_discovery` → `replay_discovery_items(path, limit) -> (Vec<TraceItem>, bool)`（稳定 `"h{ts}-{n}"` id；`DiscoveryRecord` 派生 `Deserialize` 故直接反序列化）。
  5. `api.rs` 纯函数补 `upstream_detail`、`tool_detail`、`trace_detail`（+ `TRACE_HISTORY_SCAN=50_000`，与 list 共用扫描窗口保证 id 稳定）；`traces` 返回 `Vec<TraceItem>`；`TracesResponse.traces` 类型更新。
  6. 路由表加三行：
```markdown
| GET | `/api/upstreams/{name}` | `Json<UpstreamDetail>` 或 404（其工具列表 + 计数/状态） |
| GET | `/api/tools/{name}` | `Json<ToolDetail>` 或 404（schema + 所属上游；`name`=qualified `{server}__{tool}`） |
| GET | `/api/traces/{id}` | `Json<TraceItem>` 或 404（`h…`→历史回放定位；否则 live seq 取环） |
```
  7. SPA 段补：新增 `UpstreamDetail`/`ToolDetail`/`TraceDetail` 三个详情视图 + 交叉链接（upstream↔tool↔call↔trace），Overview 卡片可点；hash 路由参数 `decodeURIComponent`。

- [ ] **Step 3: L3（`docs/L3-details/dashboard.md`）** —— READ 后：在数据来源/前端段补：traces 现像 calls 一样分配稳定 id（live 环 seq + history `"h{ts}-{n}"`，detail 与 list 共用 `TRACE_HISTORY_SCAN`），新增三个详情端点与三个详情视图，三区域形成「列表→详情→交叉跳转」闭环；测试覆盖补 `upstream_detail`/`tool_detail`/`trace_detail`/trace ring seq/`replay_discovery_items` 与 e2e 详情断言。

- [ ] **Step 4: L2（`docs/L2-components/dashboard.md`）** —— READ 后：`build_dashboard_router` → `11 个 /api/* + assets::static_handler fallback`；`/api/*` 端点列表补三个 detail 端点；`trace.rs` 组件表补 `TraceItem` + `get(seq)`、`recent` 返回 `TraceItem`。

- [ ] **Step 5: L1（`docs/L1-overview.md`）** —— READ 后：
  1. M 路线图加：`子系统 A · M3（下钻详情页）✅ —— 新增 /api/{upstreams/{name},tools/{name},traces/{id}} 三个详情端点（只读 API 增至 11）；前端 UpstreamDetail/ToolDetail/TraceDetail 三个详情视图 + 上游↔工具↔调用↔追踪交叉链接 + Overview 卡片可点；traces 像 calls 一样分配稳定 id`。
  2. 架构框 + 能力段的端点数 `8` → `11`，端点清单补三个 detail。
  3. 测试计数行用 Step 6 实测数字替换（含 dashboard 的本里程碑新增单测）。

- [ ] **Step 6: README（`docs/README.md`）** —— READ 后：子系统 A 覆盖摘要里 `8 个 /api/*` → `11 个 /api/*（含 M3 的 upstreams/{name}、tools/{name}、traces/{id} 详情下钻）`。

- [ ] **Step 7: 四道门禁（M3 验收）**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿。记录 `cargo test --all-features` 的 `N passed / M ignored`（`... | awk '/^test result:/{...}'`）回填 L1；并跑 `cargo test -p mcpgw --test dashboard -- --ignored`（1 passed）；并确认前端可独立构建且 dist 同步：`cd crates/dashboard/ui && npm run build && cd ../../.. && git status --short crates/dashboard/ui/dist`（应为空）。

- [ ] **Step 8: 提交**

```bash
git add crates/mcpgw/tests/dashboard.rs docs/L1-overview.md docs/L2-components/dashboard.md docs/L3-details/dashboard.md docs/L4-api/dashboard.md docs/README.md
git commit -m "test+docs: M3 detail e2e assertions + sync L1-L4/README (11 endpoints, drill-down detail)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## M3 完成判据（Definition of Done）

- [ ] 三个只读详情端点可用：`/api/upstreams/{name}`（工具列表+计数/状态，404）、`/api/tools/{name}`（schema+上游，404）、`/api/traces/{id}`（live seq / history 复合 id，404）。
- [ ] traces 像 calls 一样分配稳定 id（live 环 seq + history `"h{ts}-{n}"`，detail/list 共用 `TRACE_HISTORY_SCAN`）。
- [ ] 前端三个详情视图（UpstreamDetail/ToolDetail/TraceDetail）+ 列表行可点 + Overview 卡片可点 + CallDetail/TraceDetail 命中交叉链接到真实详情页；hash 路由参数 decode。
- [ ] 与 M2 相比无回归（列表视图仍工作）；每步结束 dashboard 完整可用。
- [ ] L1-L4 + README 一致更新为 11 端点；四道门禁全绿；ignored e2e（含 trace 详情 + 404）通过；dist 与源同步。

## 给实现者的备注

- **镜像 M1**：trace id/seq/replay 与 `calls.rs`/`replay_audit_calls` 完全同构——遇到设计疑问参照 calls 的既有实现。
- **DRY**：详情页「最近调用」一律复用 `/api/calls?upstream=&tool=`；不新增 calls 端点。三个详情视图共用 `getJSON` + 404-感知 raw fetch 模式（与 `CallDetail` 一致）。
- **YAGNI**：只做只读详情 + 交叉链接；**不**做写操作（M4/M5）、不做图表、不引入前端库。
- **XSS**：schema 用 `<pre>{JSON.stringify(...)}</pre>`（文本插值）；全程无 `{@html}`（测试强制）。
- **锁纪律**：trace ring 的 `Mutex` 锁内分配 seq、不跨 `.await`，`.lock().unwrap_or_else(|e| e.into_inner())`（与 calls 一致）。
- **每改 ui/src 必 `npm run build` 并提交 dist**；门禁里 dist 必须与源同步。
