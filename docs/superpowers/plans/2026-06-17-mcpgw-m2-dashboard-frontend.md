# M2：Dashboard 前端应用骨架（Svelte + Vite + rust-embed）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把现有 55 行扁平 vanilla-JS 面板替换为一个 **Svelte + Vite 构建的多视图、hash 路由的下钻应用**：左侧导航（Overview / Upstreams / Tools / Calls / Traces）+ 列表→详情下钻；产物 `dist/` 经 **rust-embed** 内嵌进二进制（`dist/` 入库、`cargo build` 不依赖 node）。M2 接通 Overview + Calls（含逐条下钻，消费 M1 的 `/api/calls`）并把现有 Upstreams/Tools/Traces 面板移植为基础列表视图（**无功能回归**）；详情页/交叉链接留给 M3。

**Architecture:** 新增 `crates/dashboard/ui/`（Svelte+Vite 工程，`npm run build` → `ui/dist/` 带 hash 多文件）。Rust 侧新增 `assets.rs` 用 `rust-embed` 内嵌 `ui/dist/`，`lib.rs` 把原来 `include_str!` 三文件 + `/app.js`/`/style.css` 路由换成「`/api/*` 显式路由 + fallback 静态资源处理器（`/`→index.html、`/assets/*`→内嵌资源）」。前端用**纯 hash 路由**（`#/overview`、`#/calls`、`#/calls/:id` …），故深链接刷新只请求 `/`，服务端无需 SPA 重写表。**不新增后端端点**——M2 全部消费已有 7 个 `/api/*`。

**Tech Stack:** Svelte 5 + Vite 5、`@sveltejs/vite-plugin-svelte`、Rust `rust-embed`（`debug-embed` 特性，始终内嵌）、axum 0.8。node v25 / npm 11 已在环境中可用。

**关键约束（务必遵守）：**
- **`dist/` 入库、`node_modules/` 不入库**：`cargo build --locked` 必须无需 node（产物已提交）。改前端的每个 task 都要 `npm run build` 重新生成 `dist/` 并随源码一起提交。
- **hash 路由**（不是 history 路由）：路由状态在 URL fragment（`#/...`），永不发给服务端 → 服务端只需 `/`+`/assets/*`，无需 history fallback 重写。
- **保持现有 `/api/*` 不变**：M2 不碰后端 API 逻辑，只换静态资源交付方式（删 `include_str!` 与 `/app.js`/`/style.css` 路由）。
- **保留 Host 反 DNS-rebinding 中间件**（`require_local_host` / `enforce_loopback_host`）——它必须照样套在新 router 上。
- **XSS**：Svelte 默认对 `{表达式}` 文本插值转义；**禁止** `{@html ...}` 用于任何来自 `/api/*` 的不可信字段（沿用现状的「所有不可信字段转义」纪律）。
- 环境是 Arch Linux：**不要自动 `npm install -g` 或装系统包**；`npm install`（项目本地、写 `ui/node_modules/`）允许。若缺依赖，提示用户而非擅自全局安装。

---

## 文件结构

| 文件/目录 | 职责 | 动作 |
|---|---|---|
| `crates/dashboard/ui/package.json` | 前端依赖与脚本（`build`/`dev`） | 新建 |
| `crates/dashboard/ui/vite.config.js` | Vite 配置（svelte 插件、`outDir: dist`、`base: '/'`） | 新建 |
| `crates/dashboard/ui/index.html` | Vite 入口 HTML（挂载点 + module script） | 新建 |
| `crates/dashboard/ui/src/main.js` | Svelte 应用挂载入口 | 新建 |
| `crates/dashboard/ui/src/App.svelte` | 根组件：壳布局 + hash 路由分发 | 新建 |
| `crates/dashboard/ui/src/lib/api.js` | `fetch /api/*` 封装 + hash 路由小工具 | 新建 |
| `crates/dashboard/ui/src/lib/*.svelte` | 各视图组件（Overview/Calls/CallDetail/Upstreams/Tools/Traces/Nav） | 新建 |
| `crates/dashboard/ui/dist/**` | 构建产物（**入库**） | 新建（生成） |
| `crates/dashboard/src/assets.rs` | `rust-embed` 内嵌 `ui/dist/` + 静态资源 axum 处理器 | 新建 |
| `crates/dashboard/src/lib.rs` | 路由装配：删旧静态路由、改用 `assets::static_handler` fallback | 修改 |
| `crates/dashboard/assets/{index.html,app.js,style.css}` | 旧扁平前端 | 删除 |
| `crates/dashboard/Cargo.toml` | 加 `rust-embed` 依赖 | 修改 |
| `.gitignore` | 忽略 `crates/dashboard/ui/node_modules/`（**不**忽略 `dist/`） | 修改 |
| `crates/mcpgw/tests/dashboard.rs` | e2e 断言 `/` 返回内嵌应用 | 修改 |
| `docs/L1-overview.md` / `docs/L2-components/dashboard.md` / `docs/L3-details/dashboard.md` / `docs/L4-api/dashboard.md` | 分层文档同步 | 修改 |

---

## Task 1：脚手架 Vite + Svelte 工程（`ui/`），产出首个 `dist/`

**Files:**
- Create: `crates/dashboard/ui/package.json`, `vite.config.js`, `index.html`, `src/main.js`, `src/App.svelte`
- Modify: `.gitignore`（加 `crates/dashboard/ui/node_modules/`）
- Generate+commit: `crates/dashboard/ui/dist/**`

本任务只建一个**能构建的最小 Svelte 应用**（渲染 "mcpgw dashboard" 字样），验证 `npm install && npm run build` 产出 `dist/`，并把工程 + 首个 `dist/` 入库。**先不接任何 API、不做路由**（那是后续 task）。

- [ ] **Step 1: 写 `package.json`**

`crates/dashboard/ui/package.json`：
```json
{
  "name": "mcpgw-dashboard-ui",
  "private": true,
  "version": "0.0.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build"
  },
  "devDependencies": {
    "@sveltejs/vite-plugin-svelte": "^4.0.0",
    "svelte": "^5.0.0",
    "vite": "^5.4.0"
  }
}
```

- [ ] **Step 2: 写 `vite.config.js`**

```js
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Output to dist/ (committed, embedded by rust-embed). base '/' so assets resolve at /assets/*
// which the Rust static handler serves from the embedded dist/.
export default defineConfig({
  plugins: [svelte()],
  base: "/",
  build: { outDir: "dist", emptyOutDir: true },
});
```

- [ ] **Step 3: 写 `index.html`、`src/main.js`、`src/App.svelte`**

`crates/dashboard/ui/index.html`:
```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>mcpgw dashboard</title>
  </head>
  <body>
    <div id="app"></div>
    <script type="module" src="/src/main.js"></script>
  </body>
</html>
```

`crates/dashboard/ui/src/main.js`:
```js
import { mount } from "svelte";
import App from "./App.svelte";

const app = mount(App, { target: document.getElementById("app") });
export default app;
```

`crates/dashboard/ui/src/App.svelte`:
```svelte
<main>
  <h1>mcpgw dashboard</h1>
  <p>loading…</p>
</main>
```

- [ ] **Step 4: gitignore node_modules（不 ignore dist）**

在仓库根 `.gitignore` 追加：
```
crates/dashboard/ui/node_modules/
```
（确认没有任何规则忽略 `dist/`；`crates/dashboard/ui/dist/` 必须可入库。）

- [ ] **Step 5: 安装依赖并构建**

Run:
```bash
cd crates/dashboard/ui && npm install && npm run build
```
Expected: `crates/dashboard/ui/dist/index.html` + `crates/dashboard/ui/dist/assets/*.js` 生成；`dist/index.html` 内引用 `/assets/...`。
（`npm install` 会写 `ui/node_modules/` 与 `ui/package-lock.json`。**package-lock.json 入库**以锁定可复现构建。）

- [ ] **Step 6: 验证产物形状**

Run: `ls crates/dashboard/ui/dist && grep -o '/assets/[^"]*' crates/dashboard/ui/dist/index.html | head`
Expected: 看到 `index.html` 与 `assets/` 目录；index.html 引用 `/assets/<hash>.js`（和可能的 `.css`）。

- [ ] **Step 7: 提交（工程 + lockfile + 首个 dist）**

```bash
git add crates/dashboard/ui/package.json crates/dashboard/ui/package-lock.json \
        crates/dashboard/ui/vite.config.js crates/dashboard/ui/index.html \
        crates/dashboard/ui/src crates/dashboard/ui/dist .gitignore
git commit -m "feat(dashboard/ui): scaffold Svelte+Vite app, commit initial dist/

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

- [ ] **Step 8: 确认 cargo 侧未受影响**

Run: `cargo build -p dashboard`
Expected: 仍然编译通过（此 task 未碰 Rust 代码；`ui/` 对 cargo 透明）。

---

## Task 2：rust-embed 内嵌 `ui/dist/` + 替换静态资源交付

**Files:**
- Modify: `crates/dashboard/Cargo.toml`（加 `rust-embed`）
- Create: `crates/dashboard/src/assets.rs`（`RustEmbed` 派生 + `static_handler`）
- Modify: `crates/dashboard/src/lib.rs`（删旧 `include_str!`/三静态路由/`asset_tests`，改用 `assets::static_handler` 作 fallback）
- Delete: `crates/dashboard/assets/{index.html,app.js,style.css}`
- Modify: `crates/mcpgw/tests/dashboard.rs`（e2e 断言 `/` 返回内嵌应用）

把「编译期 `include_str!` 三个手写文件 + `/app.js`/`/style.css` 路由」换成「`rust-embed` 内嵌 `ui/dist/` + 一个 fallback 静态处理器」。**hash 路由**意味着浏览器只会请求 `/` 和 `/assets/*`，故无需 history 重写。

- [ ] **Step 1: 加依赖**

`crates/dashboard/Cargo.toml` 的 `[dependencies]` 加：
```toml
rust-embed = { version = "8", features = ["debug-embed"] }
```
（`debug-embed`：debug 构建也始终内嵌，避免运行时按 cwd 读盘的不确定性。）

- [ ] **Step 2: 写失败测试（assets.rs）**

新建 `crates/dashboard/src/assets.rs`，先放测试：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_index_present_and_has_mount_point() {
        let idx = Assets::get("index.html").expect("ui/dist/index.html embedded");
        let html = std::str::from_utf8(idx.data.as_ref()).unwrap();
        assert!(html.contains("id=\"app\""), "index.html has the Svelte mount point");
    }

    #[test]
    fn embedded_dist_has_a_js_asset() {
        // Vite emits hashed JS under assets/. At least one must be embedded.
        let has_js = Assets::iter().any(|p| p.starts_with("assets/") && p.ends_with(".js"));
        assert!(has_js, "a hashed JS asset is embedded under assets/");
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p dashboard assets::`
Expected: 编译错误 `cannot find type Assets`（结构体未定义）。

- [ ] **Step 4: 写实现（assets.rs，放测试之上）**

```rust
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The built Svelte UI (`ui/dist/`), embedded into the binary at compile time. `dist/` is committed,
/// so `cargo build` needs no node toolchain.
#[derive(RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

/// Serve an embedded UI asset by request path: `/` -> `index.html`, `/assets/x` -> that asset.
/// Unknown paths fall back to `index.html` (harmless — the SPA uses hash routing, so real requests
/// are only `/` and `/assets/*`; this just makes a stray deep-link refresh load the app).
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(content) => {
            let mime = content.metadata.mimetype();
            (
                [(header::CONTENT_TYPE, mime.to_string())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => match Assets::get("index.html") {
            Some(index) => (
                [(header::CONTENT_TYPE, "text/html")],
                index.data.into_owned(),
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p dashboard assets::`
Expected: PASS（两个测试；依赖 Task 1 已提交的 `ui/dist/`）。

- [ ] **Step 6: 改 lib.rs —— 注册模块 + 删旧静态交付 + fallback**

在 `crates/dashboard/src/lib.rs`：
1. 模块声明区加 `mod assets;`。
2. **删除** `const INDEX_HTML/APP_JS/STYLE_CSS`（`include_str!` 三行）与 `h_index`/`h_app_js`/`h_style_css` 三个 handler，以及整个 `#[cfg(test)] mod asset_tests { ... }`（它断言旧 const）。
3. 在 `build_dashboard_router` 里**删掉** `.route("/", get(h_index))`、`.route("/app.js", get(h_app_js))`、`.route("/style.css", get(h_style_css))` 三行，改为在 `.with_state(state)` 之前加：
   ```rust
           .fallback(assets::static_handler)
   ```
4. 清理因删 handler 而不再使用的 import：`axum::response::Html`、`axum::http::header::CONTENT_TYPE`（若别处仍用则保留；用编译器/clippy 校验）。

- [ ] **Step 7: 删旧资源文件**

```bash
git rm crates/dashboard/assets/index.html crates/dashboard/assets/app.js crates/dashboard/assets/style.css
```
（`assets/` 目录可留空或一并删除；确认 `crates/dashboard/src` 不再 `include_str!` 它们。）

- [ ] **Step 8: 编译 + 全 crate 测试 + clippy**

Run: `cargo test -p dashboard && cargo clippy -p dashboard --all-targets -- -D warnings && cargo fmt -p dashboard --check`
Expected: 全过、无 warning（尤其确认无 unused-import 残留）、无 fmt diff。

- [ ] **Step 9: e2e 断言 `/` 返回内嵌应用**

在 `crates/mcpgw/tests/dashboard.rs` 的 `dashboard_serves_api_and_captures_a_trace`（`#[ignore]`）里，`client.cancel()` 之前加：
```rust
    // M2: `/` serves the embedded Svelte app (text/html with the mount point).
    let root = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(root.status(), 200);
    let ctype = root.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ctype.starts_with("text/html"), "/ is HTML, got {ctype}");
    let body = root.text().await.unwrap();
    assert!(body.contains("id=\"app\""), "/ returns the SPA mount point");
```
Run（显式跑 ignored e2e）：`cargo test -p mcpgw --test dashboard -- --ignored`
Expected: 1 passed。

- [ ] **Step 10: 提交**

```bash
git add crates/dashboard/Cargo.toml crates/dashboard/src/assets.rs crates/dashboard/src/lib.rs \
        crates/mcpgw/tests/dashboard.rs Cargo.lock
git rm crates/dashboard/assets/index.html crates/dashboard/assets/app.js crates/dashboard/assets/style.css
git commit -m "feat(dashboard): embed Svelte dist via rust-embed; drop include_str static routes

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

## Task 2 备注
- `Assets::get(path)` 返回 `Option<EmbeddedFile>`；`.data` 是 `Cow<'static,[u8]>`（用 `.into_owned()` 得 `Vec<u8>`，`Vec<u8>` impl `IntoResponse`）；`.metadata.mimetype()` 给内容类型（rust-embed 用 mime_guess 在派生时算好）。
- `#[folder = "ui/dist/"]` 相对 crate manifest 目录（`crates/dashboard/`），即 `crates/dashboard/ui/dist/` —— 必须在编译期存在（Task 1 已提交）。若 `dist/` 缺失，派生在编译期报错。
- Host 反 DNS-rebinding 中间件不动：`build_dashboard_router` 末尾的 `if enforce_loopback_host { router.layer(...) }` 照旧套在含 fallback 的 router 上。

---

## Task 3：前端壳 —— hash 路由 + 侧栏导航 + api 封装 + Overview 视图

**Files:**
- Create: `crates/dashboard/ui/src/app.css`, `src/lib/api.js`, `src/lib/router.svelte.js`, `src/lib/Nav.svelte`, `src/lib/Overview.svelte`
- Modify: `crates/dashboard/ui/src/main.js`, `src/App.svelte`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

建立应用骨架：一个最小 hash 路由（`#/<view>/<...params>`）、固定左侧导航、`fetch /api/*` 封装，并接通 **Overview** 视图（消费 `/api/overview`）。其余导航项暂时落到「coming soon」（后续 task 接通）。

> **前端测试约定**：不引入 JS 测试框架（保持依赖最小）。每个前端 task 的验证 = `npm run build` 成功 + 重生成 `dist/` 提交 + Rust 侧 `cargo test -p dashboard`（内嵌断言：`index.html` 仍含挂载点、`assets/` 仍有 JS）仍过。逻辑正确性由后端 API 测试 + ignored e2e 覆盖。

- [ ] **Step 1: `src/lib/api.js`**

```js
/** GET a JSON endpoint; throws on non-2xx. */
export async function getJSON(path) {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path} -> ${r.status}`);
  return r.json();
}
```

- [ ] **Step 2: `src/lib/router.svelte.js`（Svelte 5 runes 模块）**

```js
// Tiny hash router: `#/<view>/<...params>` -> reactive { view, params }. Hash routing means the
// fragment is never sent to the server, so deep-link refresh only ever requests `/`.
function parse() {
  const raw = window.location.hash.replace(/^#\/?/, "");
  const parts = raw.split("/").filter(Boolean);
  return { view: parts[0] || "overview", params: parts.slice(1) };
}

export const route = $state(parse());

export function startRouter() {
  const update = () => {
    const r = parse();
    route.view = r.view;
    route.params = r.params;
  };
  window.addEventListener("hashchange", update);
  update();
}
```

- [ ] **Step 3: `src/lib/Nav.svelte`**

```svelte
<script>
  import { route } from "./router.svelte.js";
  const items = [
    ["overview", "Overview"],
    ["upstreams", "Upstreams"],
    ["tools", "Tools"],
    ["calls", "Calls"],
    ["traces", "Traces"],
  ];
</script>

<nav class="sidebar">
  <div class="brand">mcpgw</div>
  <ul>
    {#each items as [view, label]}
      <li class:active={route.view === view}><a href={`#/${view}`}>{label}</a></li>
    {/each}
  </ul>
</nav>
```

- [ ] **Step 4: `src/lib/Overview.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let data = $state(null);
  let error = $state(null);
  async function load() {
    try { data = await getJSON("/api/overview"); error = null; }
    catch (e) { error = String(e); }
  }
  onMount(() => {
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  });
</script>

<h2>Overview</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if data}
  <div class="cards">
    <div class="card"><div class="label">upstreams</div><div class="v">{data.upstreams_connected}/{data.upstreams_total}</div></div>
    <div class="card"><div class="label">tools</div><div class="v">{data.tools_total}</div></div>
    <div class="card"><div class="label">calls</div><div class="v">{data.total_calls}</div></div>
    <div class="card"><div class="label">strategy</div><div class="v">{data.strategy}</div></div>
    <div class="card"><div class="label">uptime</div><div class="v">{data.uptime_secs}s</div></div>
    <div class="card"><div class="label">rebuild skipped</div><div class="v">{data.last_rebuild_skipped}</div></div>
  </div>
{:else if !error}
  <p class="muted">loading…</p>
{/if}
```

- [ ] **Step 5: `src/main.js` + `src/App.svelte`**

`src/main.js`:
```js
import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

const app = mount(App, { target: document.getElementById("app") });
export default app;
```

`src/App.svelte`:
```svelte
<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import Nav from "./lib/Nav.svelte";
  import Overview from "./lib/Overview.svelte";
  onMount(startRouter);
</script>

<div class="layout">
  <Nav />
  <main class="content">
    {#if route.view === "overview"}
      <Overview />
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
```

- [ ] **Step 6: `src/app.css`（壳布局 + 共用基础样式）**

```css
:root { --bg:#0f1115; --panel:#171a21; --fg:#e6e6e6; --muted:#8a93a2; --accent:#4f8cff; --border:#262b36; }
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--fg); font:14px/1.5 system-ui, sans-serif; }
a { color:var(--accent); text-decoration:none; }
.layout { display:grid; grid-template-columns:200px 1fr; min-height:100vh; }
.sidebar { background:var(--panel); border-right:1px solid var(--border); padding:16px 0; }
.sidebar .brand { font-weight:700; font-size:18px; padding:0 16px 12px; }
.sidebar ul { list-style:none; margin:0; padding:0; }
.sidebar li a { display:block; padding:8px 16px; color:var(--fg); }
.sidebar li.active a, .sidebar li a:hover { background:#212634; color:var(--accent); }
.content { padding:20px 24px; }
.muted, .label { color:var(--muted); }
.error { color:#ff6b6b; }
.cards { display:flex; flex-wrap:wrap; gap:12px; }
.card { background:var(--panel); border:1px solid var(--border); border-radius:8px; padding:12px 16px; min-width:120px; }
.card .v { font-size:22px; font-weight:600; }
table { width:100%; border-collapse:collapse; }
th, td { text-align:left; padding:6px 10px; border-bottom:1px solid var(--border); }
th { color:var(--muted); font-weight:500; }
.row-link { cursor:pointer; }
.row-link:hover { background:#1c2230; }
.badge { padding:1px 6px; border-radius:4px; font-size:12px; }
.badge.connected { background:#16361f; color:#5fd38d; }
.badge.skipped, .badge.unknown { background:#3a2f16; color:#e2b04a; }
.chips { display:flex; gap:8px; flex-wrap:wrap; margin:8px 0 12px; }
.chip { padding:3px 10px; border:1px solid var(--border); border-radius:14px; cursor:pointer; }
.chip.active { background:var(--accent); color:#fff; border-color:var(--accent); }
```

- [ ] **Step 7: 构建并重生成 dist**

Run: `cd crates/dashboard/ui && npm run build`
Expected: 构建成功；`dist/` 更新（新 hash 文件名）。

- [ ] **Step 8: Rust 内嵌断言仍过**

Run: `cargo test -p dashboard assets::`
Expected: PASS（`index.html` 仍含 `id="app"`、`assets/` 仍有 JS）。

- [ ] **Step 9: 提交（源 + 重生成 dist）**

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): app shell — hash router, sidebar nav, Overview view

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：Calls 下钻 —— meta 指标摘要 + 逐条列表 + 单条详情

**Files:**
- Create: `crates/dashboard/ui/src/lib/Calls.svelte`, `src/lib/CallDetail.svelte`
- Modify: `crates/dashboard/ui/src/App.svelte`（接入 `calls` 与 `calls/:id` 路由）
- Regenerate+commit: `crates/dashboard/ui/dist/**`

实现核心下钻链：Calls 页顶部是**可点的 meta-tool 指标卡**（`/api/metrics` 的 `per_meta_tool`：calls/err/p50/p95），点某卡即把下方**逐条调用列表**（`/api/calls`）按该 meta 过滤；列表每行可点进 `#/calls/<id>` 的**单条详情**（`/api/calls/{id}`）。支持 source（live/history）、outcome 过滤与分页。

- [ ] **Step 1: `src/lib/Calls.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";

  const LIMIT = 50;
  let metrics = $state([]);     // per_meta_tool summary
  let source = $state("live");  // live | history
  let meta = $state("");        // "" = all meta-tools
  let outcome = $state("");     // "" = all outcomes
  let offset = $state(0);
  let resp = $state(null);      // CallsResponse
  let error = $state(null);

  const query = $derived.by(() => {
    const q = new URLSearchParams();
    q.set("source", source);
    if (meta) q.set("meta", meta);
    if (outcome) q.set("outcome", outcome);
    q.set("limit", String(LIMIT));
    q.set("offset", String(offset));
    return q.toString();
  });

  async function loadMetrics() {
    try { const m = await getJSON("/api/metrics"); metrics = m.per_meta_tool ?? []; } catch (_) {}
  }
  async function loadCalls() {
    try { resp = await getJSON(`/api/calls?${query}`); error = null; }
    catch (e) { error = String(e); }
  }
  function pickMeta(m) { meta = meta === m ? "" : m; offset = 0; }
  function setSource(s) { source = s; offset = 0; }
  function setOutcome(o) { outcome = outcome === o ? "" : o; offset = 0; }
  function when(ms) { return new Date(ms).toLocaleString(); }

  // Refetch the list whenever any filter changes (reading `query` tracks all of them).
  $effect(() => { void query; loadCalls(); });
  onMount(() => {
    loadMetrics();
    const t = setInterval(() => { loadMetrics(); loadCalls(); }, 3000);
    return () => clearInterval(t);
  });
</script>

<h2>Calls</h2>

<div class="cards">
  {#each metrics as m}
    <div class="card row-link" class:active={meta === m.meta_tool} onclick={() => pickMeta(m.meta_tool)}>
      <div class="label">{m.meta_tool}</div>
      <div class="v">{m.calls}</div>
      <div class="muted">err {m.errors} · p50 {m.p50_ms}ms · p95 {m.p95_ms}ms</div>
    </div>
  {/each}
</div>

<div class="chips">
  <span class="chip" class:active={source === "live"} onclick={() => setSource("live")}>live</span>
  <span class="chip" class:active={source === "history"} onclick={() => setSource("history")}>history</span>
  <span class="muted">·</span>
  {#each ["ok", "error", "timeout"] as o}
    <span class="chip" class:active={outcome === o} onclick={() => setOutcome(o)}>{o}</span>
  {/each}
  {#if meta}<span class="chip active" onclick={() => pickMeta(meta)}>meta: {meta} ✕</span>{/if}
</div>

{#if error}<p class="error">{error}</p>{/if}
{#if resp}
  {#if resp.source === "history" && resp.history_unavailable}
    <p class="muted">history unavailable (enable [audit])</p>
  {:else}
    <p class="muted">{resp.total} total</p>
    <table>
      <thead><tr><th>time</th><th>meta</th><th>target</th><th>upstream</th><th>outcome</th><th>ms</th></tr></thead>
      <tbody>
        {#each resp.items as c}
          <tr class="row-link" onclick={() => (location.hash = `#/calls/${c.id}`)}>
            <td>{when(c.ts_unix_ms)}</td>
            <td>{c.meta_tool}</td>
            <td>{c.target_tool ?? "—"}</td>
            <td>{c.upstream ?? "—"}</td>
            <td>{c.outcome}</td>
            <td>{c.latency_ms}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="chips">
      <span class="chip" class:disabled={offset === 0} onclick={() => (offset = Math.max(0, offset - LIMIT))}>‹ prev</span>
      <span class="muted">{offset + 1}–{Math.min(offset + LIMIT, resp.total)}</span>
      <span class="chip" class:disabled={offset + LIMIT >= resp.total} onclick={() => { if (offset + LIMIT < resp.total) offset += LIMIT; }}>next ›</span>
    </div>
  {/if}
{:else if !error}
  <p class="muted">loading…</p>
{/if}
```

- [ ] **Step 2: `src/lib/CallDetail.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let { id } = $props();
  let item = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      const r = await fetch(`/api/calls/${encodeURIComponent(id)}`);
      if (r.status === 404) { notFound = true; item = null; return; }
      if (!r.ok) throw new Error(`/api/calls/${id} -> ${r.status}`);
      item = await r.json(); error = null;
    } catch (e) { error = String(e); }
  }
  onMount(load);
  function when(ms) { return new Date(ms).toLocaleString(); }
</script>

<p><a href="#/calls">‹ back to Calls</a></p>
<h2>Call detail</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">call not found (it may have aged out of the live ring)</p>
{:else if item}
  <table>
    <tbody>
      <tr><th>id</th><td>{item.id}</td></tr>
      <tr><th>time</th><td>{when(item.ts_unix_ms)}</td></tr>
      <tr><th>meta_tool</th><td>{item.meta_tool}</td></tr>
      <tr><th>target_tool</th><td>{#if item.target_tool}<a href="#/tools">{item.target_tool}</a>{:else}—{/if}</td></tr>
      <tr><th>upstream</th><td>{#if item.upstream}<a href="#/upstreams">{item.upstream}</a>{:else}—{/if}</td></tr>
      <tr><th>outcome</th><td>{item.outcome}</td></tr>
      <tr><th>error_kind</th><td>{item.error_kind ?? "—"}</td></tr>
      <tr><th>latency_ms</th><td>{item.latency_ms}</td></tr>
      <tr><th>arg_bytes</th><td>{item.arg_bytes}</td></tr>
      <tr><th>result_bytes</th><td>{item.result_bytes}</td></tr>
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
```

> 详情里 `upstream`/`target_tool` 暂时链到列表页（`#/upstreams`、`#/tools`）；M3 会改成带过滤的详情页交叉链接。`{表达式}` 自动转义，无 `{@html}`，不引入 XSS。

- [ ] **Step 3: `src/App.svelte` 接入 calls 路由**

把 `App.svelte` 的路由分发改为（在 Overview 分支后加 calls 与 calls/:id）：
```svelte
<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import Nav from "./lib/Nav.svelte";
  import Overview from "./lib/Overview.svelte";
  import Calls from "./lib/Calls.svelte";
  import CallDetail from "./lib/CallDetail.svelte";
  onMount(startRouter);
</script>

<div class="layout">
  <Nav />
  <main class="content">
    {#if route.view === "overview"}
      <Overview />
    {:else if route.view === "calls" && route.params.length > 0}
      <CallDetail id={route.params[0]} />
    {:else if route.view === "calls"}
      <Calls />
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
```

- [ ] **Step 4: 构建 + Rust 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd - && cargo test -p dashboard assets::`
Expected: 构建成功；`assets::` 内嵌测试仍过。

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): Calls drill-down — metric cards, per-call list, call detail

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

- [ ] **Step 5: 手动冒烟（可选但推荐，验证下钻链真的通）**

用 M1 演示配置起 `mcpgw serve`（dashboard 开、HTTP 下游开、一个 mock 上游），驱动几次 call_tool/search_tools，然后浏览器开 `http://127.0.0.1:<dash port>/#/calls`：确认指标卡可点过滤、列表行可点进详情、详情显示元数据、404 文案在不存在 id 时出现。**仅手动验证，不写自动化前端测试。**

---

## Task 5：移植 Upstreams / Tools / Traces 基础视图（消除回归）

**Files:**
- Create: `crates/dashboard/ui/src/lib/Upstreams.svelte`, `src/lib/Tools.svelte`, `src/lib/Traces.svelte`
- Modify: `crates/dashboard/ui/src/App.svelte`（接入三个路由）
- Regenerate+commit: `crates/dashboard/ui/dist/**`

把旧扁平面板的三块（上游表、工具列表、查询追踪）移植成新壳里的基础列表视图，**功能与现状持平**（详情页/交叉链接是 M3）。

- [ ] **Step 1: `src/lib/Upstreams.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let ups = $state([]);
  let error = $state(null);
  async function load() {
    try { ups = await getJSON("/api/upstreams"); error = null; }
    catch (e) { error = String(e); }
  }
  onMount(() => { load(); const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Upstreams</h2>
{#if error}<p class="error">{error}</p>{/if}
<table>
  <thead><tr><th>name</th><th>transport</th><th>status</th><th>tools</th><th>calls</th><th>errors</th></tr></thead>
  <tbody>
    {#each ups as u}
      <tr>
        <td>{u.name}</td>
        <td>{u.transport}</td>
        <td><span class="badge {u.status}">{u.status}</span>{#if u.reason} {u.reason}{/if}</td>
        <td>{u.tools}</td>
        <td>{u.calls}</td>
        <td>{u.errors}</td>
      </tr>
    {/each}
  </tbody>
</table>
```

- [ ] **Step 2: `src/lib/Tools.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let q = $state("");
  let tools = $state([]);
  let error = $state(null);
  async function load() {
    try {
      const qs = q ? `?q=${encodeURIComponent(q)}` : "";
      tools = await getJSON(`/api/tools${qs}`); error = null;
    } catch (e) { error = String(e); }
  }
  $effect(() => { void q; load(); });
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Tools</h2>
<input class="search" placeholder="filter tools…" bind:value={q} />
{#if error}<p class="error">{error}</p>{/if}
<p class="muted">{tools.length} tools</p>
<table>
  <thead><tr><th>name</th><th>description</th></tr></thead>
  <tbody>
    {#each tools as t}
      <tr><td>{t.name}</td><td>{t.description}</td></tr>
    {/each}
  </tbody>
</table>
```

> 给 `app.css` 追加一条 `.search { background:var(--panel); color:var(--fg); border:1px solid var(--border); border-radius:6px; padding:6px 10px; margin-bottom:10px; width:260px; }`（在本 task 顺手加进 `src/app.css`）。

- [ ] **Step 3: `src/lib/Traces.svelte`**

```svelte
<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let source = $state("live");
  let resp = $state(null);
  let error = $state(null);
  async function load() {
    try { resp = await getJSON(`/api/traces?limit=50&source=${source}`); error = null; }
    catch (e) { error = String(e); }
  }
  $effect(() => { void source; load(); });
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Query traces</h2>
<div class="chips">
  <span class="chip" class:active={source === "live"} onclick={() => (source = "live")}>live</span>
  <span class="chip" class:active={source === "history"} onclick={() => (source = "history")}>history</span>
</div>
{#if error}<p class="error">{error}</p>{/if}
{#if resp}
  {#if resp.history_unavailable}
    <p class="muted">history unavailable (enable [dashboard].trace_path)</p>
  {:else}
    {#each resp.traces as t}
      <div class="card" style="display:block;width:100%;margin-bottom:8px;">
        <div class="label">{t.query}</div>
        <div>{#each t.results as h}<span class="chip">{h.name} ({h.score.toFixed(2)})</span> {/each}</div>
      </div>
    {/each}
  {/if}
{:else if !error}
  <p class="muted">loading…</p>
{/if}
```

- [ ] **Step 4: `src/App.svelte` 接入三视图**

把 App 路由分发补全（在 calls 分支后、`coming soon` 兜底前）：
```svelte
    {:else if route.view === "upstreams"}
      <Upstreams />
    {:else if route.view === "tools"}
      <Tools />
    {:else if route.view === "traces"}
      <Traces />
```
并在 `<script>` import：
```svelte
  import Upstreams from "./lib/Upstreams.svelte";
  import Tools from "./lib/Tools.svelte";
  import Traces from "./lib/Traces.svelte";
```

- [ ] **Step 5: 构建 + 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd - && cargo test -p dashboard assets::`
Expected: 构建成功；内嵌断言仍过。

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): port Upstreams/Tools/Traces basic views (no regression)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6：分层文档同步 + 四道门禁验证

**Files:**
- Modify: `docs/L4-api/dashboard.md`（源文件列表加 `assets.rs` + `ui/`；删 `/app.js`/`/style.css`/`h_index` 等旧静态路由记录，改记 `assets::static_handler` fallback；重写「SPA」段为 Svelte+Vite+rust-embed）
- Modify: `docs/L3-details/dashboard.md`（前端/构建链段：Svelte+Vite → dist → rust-embed、dist 入库、hash 路由、视图清单）
- Modify: `docs/L2-components/dashboard.md`（依赖加 `rust-embed`；静态交付与「不负责」段更新；`build_dashboard_router` 路由说明）
- Modify: `docs/L1-overview.md`（M2 路线图一句；测试计数回填）

> 文档须忠实反映代码。本任务不改代码，最后跑全门禁确认 M2 整体绿。

- [ ] **Step 1: 更新 L4（`docs/L4-api/dashboard.md`）**

先 READ 当前文件。要点：
1. 顶部源文件列表 `{lib,metrics,trace,history,calls,api}.rs` → 加 `assets`：`{lib,metrics,trace,history,calls,api,assets}.rs` + `ui/`（Svelte+Vite 工程，产物 `ui/dist/` 入库）。
2. 新增 `## assets.rs：内嵌 UI 资源` 段：
```markdown
## `assets.rs`：内嵌 UI 资源（rust-embed）

### `struct Assets`（`#[derive(RustEmbed)] #[folder = "ui/dist/"]`）
编译期把构建好的 Svelte 产物 `ui/dist/`（带 hash 多文件）内嵌进二进制（`debug-embed`：debug 也内嵌）。`dist/` 入库，故 `cargo build` 不需 node。

### `static_handler`
```rust
pub async fn static_handler(uri: Uri) -> Response
```
按请求路径返回内嵌资源：`/`→`index.html`、`/assets/x`→该文件（`Content-Type` 取 `EmbeddedFile.metadata.mimetype()`）；未知路径回退 `index.html`（hash 路由下真实请求只有 `/` 与 `/assets/*`，回退仅为深链接刷新兜底）。装为 router 的 `.fallback(...)`。
```
3. 路由表：**删除** `/`、`/app.js`、`/style.css` 三行（旧 `include_str!` 交付），改加一行说明：
```markdown
| —（fallback） | 任意非 `/api/*` 路径 | `assets::static_handler`：`/`→内嵌 `index.html`、`/assets/*`→内嵌资源（带 hash），其余回退 index |
```
4. **删除/重写**原「SPA（`assets/app.js` + ...）」小节为：
```markdown
### 前端 SPA（`ui/`：Svelte 5 + Vite，产物内嵌）
零后端模板的单页应用：左侧导航（Overview/Upstreams/Tools/Calls/Traces）+ **hash 路由**（`#/<view>/<...params>`，如 `#/calls`、`#/calls/{id}`）。各视图轮询既有 `/api/*`；Calls 页用 `/api/metrics` 的可点指标卡过滤 `/api/calls` 逐条列表、行点进 `/api/calls/{id}` 详情。Svelte `{表达式}` 默认转义、无 `{@html}`，不引入 XSS。源在 `ui/src/`，`npm run build`→`ui/dist/`（入库），经 `assets.rs` 内嵌。
```

- [ ] **Step 2: 更新 L3（`docs/L3-details/dashboard.md`）**

READ 后，在合适位置（如「数据来源」附近或新增「前端与构建」段）加：前端从 55 行 vanilla-JS 升级为 **Svelte 5 + Vite** 多视图 hash-路由应用；构建链 `npm run build → ui/dist/`（带 hash 多文件，**入库**）→ `rust-embed` 编译期内嵌 → `cargo build --locked` 无需 node；静态交付从 `include_str!` 三文件改为 `assets::static_handler` fallback（`/`+`/assets/*`，hash 路由免 history 重写）；视图清单：Overview（卡片）、Calls（指标卡→逐条列表→详情下钻）、Upstreams/Tools/Traces（基础列表，详情页待 M3）。在「测试覆盖」补 `assets.rs` 内嵌断言（index 有挂载点、dist 有 JS）。

- [ ] **Step 3: 更新 L2（`docs/L2-components/dashboard.md`）**

READ 后：依赖段加 `rust-embed`（内嵌 `ui/dist/`）；`build_dashboard_router` 说明改为「8 个 `/api/*` 路由 + `assets::static_handler` fallback（内嵌 SPA），enforce_loopback_host 时挂 Host 校验」；「不负责」里「图表库/SSE/WebSocket」「零依赖原生 JS」一句更新为「SPA 用 Svelte+Vite 构建、产物内嵌；仍每 3s 轮询、无 SSE/WS」；新增一句构建工作流（`ui/` 工程、`npm run build` 再生 `dist/`、`dist` 入库故 cargo 不依赖 node）。

- [ ] **Step 4: 更新 L1（`docs/L1-overview.md`）**

1. M 路线图加：`子系统 A · M2（前端应用骨架）✅ —— vanilla-JS 扁平面板升级为 Svelte 5 + Vite 多视图 hash-路由应用（rust-embed 内嵌 ui/dist/、dist 入库故 cargo 不依赖 node）；接通 Overview + Calls 下钻（指标卡→逐条列表→详情）并移植 Upstreams/Tools/Traces 基础视图（无回归）`。
2. 测试计数行：用 Step 5 实测数字替换（M2 预计 +2：dashboard `assets::` 两个内嵌断言；e2e 仍 ignored）。

- [ ] **Step 5: 四道门禁（M2 验收）**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿。记录 `cargo test --all-features` 的 `N passed / M ignored` 回填 L1。**另外**显式跑一次 ignored e2e 确认前端 `/` 正常：`cargo test -p mcpgw --test dashboard -- --ignored`（应 1 passed）。**再**确认前端可独立构建：`cd crates/dashboard/ui && npm run build`（成功、dist 与已提交一致或重新提交）。

- [ ] **Step 6: 回填计数并提交**

```bash
git add docs/L1-overview.md docs/L2-components/dashboard.md docs/L3-details/dashboard.md docs/L4-api/dashboard.md
git commit -m "docs: sync L1-L4 for M2 Svelte+Vite frontend (rust-embed, drill-down shell)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## M2 完成判据（Definition of Done）

- [ ] `ui/` Svelte+Vite 工程可 `npm install && npm run build` 产出 `dist/`；`node_modules/` 不入库、`dist/` 与 `package-lock.json` 入库。
- [ ] `rust-embed` 内嵌 `ui/dist/`；`assets::static_handler` 作 fallback 交付 `/` 与 `/assets/*`；旧 `include_str!` 三文件与 `/app.js`/`/style.css` 路由已删。
- [ ] 壳：侧栏导航 + hash 路由（`#/overview`、`#/calls`、`#/calls/{id}`、`#/upstreams`、`#/tools`、`#/traces`）。
- [ ] Overview 卡片、Calls 下钻（指标卡过滤→逐条列表→单条详情 + 404 文案）、Upstreams/Tools/Traces 基础视图均工作；与现状相比无功能回归。
- [ ] `cargo build --locked` 无需 node（dist 已提交）；四道门禁全绿；ignored e2e（含 `/` 返回内嵌应用）`--ignored` 通过。
- [ ] L1-L4 文档与代码一致（静态交付方式、前端栈、构建工作流、测试计数）。

## 给实现者的备注

- **DRY / 组件边界**：每个视图一个 `.svelte`，共用 `api.js`（fetch）与 `router.svelte.js`（路由）。勿在多个组件里重复 fetch/escape 逻辑。
- **YAGNI**：M2 不做详情交叉链接的目标页（M3）、不做写操作（M4/M5）、不引入前端测试框架、不引入状态管理库。仅 Svelte+Vite+rust-embed。
- **XSS 纪律**：只用 `{表达式}`（自动转义），**绝不** `{@html}` 渲染任何 `/api/*` 字段。
- **轮询**：各视图 `onMount` 起 3s `setInterval`、卸载清除；与旧面板节奏一致。
- **dist 同步**：任何改 `ui/src/` 的提交都必须同时 `npm run build` 并提交 `ui/dist/`，否则内嵌的是旧前端。Task 6 门禁里 `npm run build` 应与已提交 `dist/` 一致。
- **Arch 环境**：`npm install` 写项目本地 `node_modules/` 可行；**勿** `npm install -g` 或动系统包；缺依赖提示用户。
- **a11y 警告非致命**：可点的 `<div>`/`<span>`（指标卡、chip、表格行）会触发 Svelte 的 a11y 警告（如 `click without keyboard handler`）。Vite 构建默认**不因警告失败**——M2 接受这些警告（详情交互/键盘可达性可在后续打磨）；**不要**为消除警告而改动逻辑或引入复杂处理。
