# Dashboard 配置编辑器：结构化表单模式 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 dashboard 的 Config 页增加结构化表单模式，与现有 raw TOML 编辑并存、两视图实时双向同步，复用现有 `PUT /api/admin/config`（后端零改动）。

**Architecture:** 全部增量在 `crates/dashboard/ui`。前端用 **smol-toml** 在 `raw 文本(TOML)` ⇄ `model(JS 对象)` 间双向转换；硬编码全段表单（布局 A：左段导航 + 右表单）；可测逻辑（`toml.js` / `validate.js`）抽成纯函数由 **vitest** 覆盖；Svelte 组件只做数据绑定。后端 `crates/dashboard/src/*` 与 API 不动。

**Tech Stack:** Svelte 5（runes：`$state`/`$derived`/`$props`）、Vite 5、smol-toml 1.7、vitest 2。

---

## 前端 model 形状（smol-toml `parse` 结果——所有 Task 的共同契约）

前端 model 使用 **TOML 原始键名**（不是 Rust serde 重命名后的名字）。验证自 smol-toml 1.7.0：

```js
// 所有顶层段都可缺省（TOML 可省略 → model 中该键为 undefined）
{
  retrieval?: {
    strategy: "bm25" | "vector" | "subagent",
    top_k: number,
    vector?:   { base_url?: string, model: string, api_key_env: string, dim?: number, timeout_ms?: number, batch_size?: number },
    subagent?: { base_url?: string, model: string, api_key_env: string, timeout_ms?: number, candidates?: number },
  },
  server?: {
    stdio: boolean,
    http?: { enabled: boolean, bind: string, path: string, api_key?: Array<{ name: string, env: string }> },  // 注意键名是 api_key（非 api_keys）
  },
  audit?: { enabled: boolean, path: string },
  dashboard?: {
    enabled: boolean, bind: string, trace_queries: boolean, trace_path?: string,
    trace_buffer: number, call_buffer: number, payload_max_bytes: number,
    admin_token_env?: string, disabled_state_path?: string,
  },
  upstream?: Array<{   // 注意键名是 upstream（非 upstreams）；transport 字段是 flatten 的
    name: string, call_timeout_ms: number,
    transport: "stdio" | "http",
    // stdio: command(必填) + args[] + env_passthrough[]
    command?: string, args?: string[], env_passthrough?: string[],
    // http: url(必填) + bearer_env? + headers?
    url?: string, bearer_env?: string, headers?: Record<string,string>,
  }>,
}
```

**段→热重载映射**（前端用于段标注）：`upstream` = 🔥热生效；`retrieval`/`server`/`audit`/`dashboard` = ⟳需重启。

---

## File Structure

新增/修改文件（全部在 `crates/dashboard/ui/`）：

| 文件 | 职责 | Task |
| --- | --- | --- |
| `package.json` | 加 `smol-toml` dep、`vitest` devDep、`"test": "vitest run"` | 1 |
| `vite.config.js` | 改用 `vitest/config` 的 `defineConfig` + `test` 字段（node 环境） | 1 |
| `src/lib/toml.js`（新） | `parseToml(raw)`/`stringifyToml(model)`：包装 smol-toml + 解析错误结构化 | 2 |
| `src/lib/toml.test.js`（新） | round-trip 单测 | 2 |
| `src/lib/configSchema.js`（新） | 枚举值、必填、段热/重启标注等字段元数据 | 3 |
| `src/lib/validate.js`（新） | `validateModel(model)`：纯函数字段级校验 → 错误列表 | 3 |
| `src/lib/validate.test.js`（新） | 校验 corner cases 单测 | 3 |
| `src/lib/RawEditor.svelte`（新） | 抽出现有 `<textarea>` raw 编辑 | 4 |
| `src/lib/Config.svelte`（改） | 容器：`Raw│Form` 切换 + Save/Reload + 同步 + 结果卡片 | 4,7 |
| `src/lib/FormEditor.svelte`（新） | 布局 A：左段导航 + 右段渲染 | 5 |
| `src/lib/SectionRetrieval.svelte`（新） | retrieval 段表单（strategy/top_k/vector/subagent） | 5 |
| `src/lib/SectionServer.svelte`（新） | server 段表单（stdio/http/api_key[]） | 5 |
| `src/lib/SectionAudit.svelte`（新） | audit 段表单 | 5 |
| `src/lib/SectionDashboard.svelte`（新） | dashboard 段表单 | 5 |
| `src/lib/SectionUpstreams.svelte`（新） | upstream 数组：增删条目 + transport 切换 | 6 |
| `src/app.css`（改） | 表单/导航/字段样式 | 8 |
| `dist/`（重建） | `npm run build` 重新生成、提交 | 8 |
| `docs/L1–L4`（改） | 同步"结构化表单模式" | 9 |

后端 `crates/dashboard/src/*`、`admin.svelte.js`、`api.js`：**不动**。

---

## Task 1: 测试设施 + 依赖（vitest + smol-toml）

**Files:**
- Modify: `crates/dashboard/ui/package.json`
- Modify: `crates/dashboard/ui/vite.config.js`
- Create: `crates/dashboard/ui/src/lib/smoke.test.js`

- [ ] **Step 1: 改 `package.json` 加依赖与 test 脚本**

```json
{
  "name": "mcpgw-dashboard-ui",
  "private": true,
  "version": "0.0.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "test": "vitest run"
  },
  "dependencies": {
    "smol-toml": "^1.7.0"
  },
  "devDependencies": {
    "@sveltejs/vite-plugin-svelte": "^4.0.0",
    "svelte": "^5.0.0",
    "vite": "^5.4.0",
    "vitest": "^2.1.0"
  }
}
```

- [ ] **Step 2: 改 `vite.config.js` 启用 vitest（node 环境，纯 JS 测试）**

```js
/// <reference types="vitest/config" />
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Output to dist/ (committed, embedded by rust-embed). base '/' so assets resolve at /assets/*
// which the Rust static handler serves from the embedded dist/.
export default defineConfig({
  plugins: [svelte()],
  base: "/",
  build: { outDir: "dist", emptyOutDir: true },
  // vitest: only the pure-JS helpers (toml.js / validate.js) are unit-tested, so a node
  // environment is enough (no jsdom / Svelte component rendering).
  test: { environment: "node", include: ["src/**/*.test.js"] },
});
```

- [ ] **Step 3: 安装依赖**

Run: `cd crates/dashboard/ui && npm install`
Expected: 新增 `smol-toml` + `vitest` 到 `node_modules`，`package-lock.json` 更新；无 error。

- [ ] **Step 4: 写 smoke 测试确认 vitest 跑通**

Create `crates/dashboard/ui/src/lib/smoke.test.js`:

```js
import { test, expect } from "vitest";

test("vitest runs", () => {
  expect(1 + 1).toBe(2);
});
```

- [ ] **Step 5: 跑测试**

Run: `cd crates/dashboard/ui && npm run test`
Expected: PASS（1 passed），vitest 正常退出 0。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/ui/package.json crates/dashboard/ui/vite.config.js crates/dashboard/ui/package-lock.json crates/dashboard/ui/src/lib/smoke.test.js
git commit -m "test(dashboard/ui): 引入 vitest + smol-toml 测试设施"
```

---

## Task 2: `toml.js` — TOML ⇄ model 双向转换

**Files:**
- Create: `crates/dashboard/ui/src/lib/toml.js`
- Create: `crates/dashboard/ui/src/lib/toml.test.js`

- [ ] **Step 1: 写失败测试**

Create `crates/dashboard/ui/src/lib/toml.test.js`:

```js
import { test, expect } from "vitest";
import { parseToml, stringifyToml } from "./toml.js";

const SAMPLE = `[retrieval]
strategy = "bm25"
top_k = 10

[retrieval.vector]
model = "e5"
api_key_env = "VK"
dim = 768

[server]
stdio = false

[server.http]
enabled = true
bind = "127.0.0.1:8970"
path = "/mcp"

[[server.http.api_key]]
name = "a"
env = "K"

[[upstream]]
name = "mock"
transport = "stdio"
command = "/bin/mock"
args = ["--x"]
env_passthrough = ["PATH", "HOME"]
call_timeout_ms = 30000

[[upstream]]
name = "remote"
transport = "http"
url = "https://x/mcp"
bearer_env = "TKN"
`;

test("parseToml returns ok model with TOML-native keys", () => {
  const r = parseToml(SAMPLE);
  expect(r.ok).toBe(true);
  expect(r.model.retrieval.strategy).toBe("bm25");
  expect(r.model.retrieval.top_k).toBe(10);
  expect(r.model.retrieval.vector.dim).toBe(768);
  expect(r.model.server.http.api_key[0].env).toBe("K");
  expect(r.model.upstream).toHaveLength(2);
  expect(r.model.upstream[0].transport).toBe("stdio");
  expect(r.model.upstream[1].url).toBe("https://x/mcp");
});

test("round-trip parse→stringify→parse is semantically equal", () => {
  const a = parseToml(SAMPLE);
  const out = stringifyToml(a.model);
  const b = parseToml(out);
  expect(b.ok).toBe(true);
  expect(b.model).toEqual(a.model);
});

test("parseToml returns structured error on invalid TOML", () => {
  const r = parseToml("this is = = not toml");
  expect(r.ok).toBe(false);
  expect(typeof r.error).toBe("string");
  expect(r.error.length).toBeGreaterThan(0);
});

test("stringifyToml on empty model yields empty-ish TOML that re-parses", () => {
  const out = stringifyToml({});
  expect(parseToml(out)).toEqual({ ok: true, model: {} });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd crates/dashboard/ui && npm run test -- toml`
Expected: FAIL（`Failed to resolve import "./toml.js"` / `parseToml is not a function`）。

- [ ] **Step 3: 实现 `toml.js`**

Create `crates/dashboard/ui/src/lib/toml.js`:

```js
import { parse, stringify } from "smol-toml";

/**
 * Parse raw TOML text into a model object (TOML-native keys: `upstream`, `api_key`).
 * Returns { ok: true, model } or { ok: false, error } — never throws.
 */
export function parseToml(raw) {
  try {
    return { ok: true, model: parse(raw) };
  } catch (e) {
    return { ok: false, error: e?.message ?? String(e) };
  }
}

/**
 * Serialize a model back to canonical TOML text.
 * NOTE: comments / original formatting are NOT preserved (normalized output).
 */
export function stringifyToml(model) {
  return stringify(model);
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd crates/dashboard/ui && npm run test -- toml`
Expected: PASS（4 passed）。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/ui/src/lib/toml.js crates/dashboard/ui/src/lib/toml.test.js
git commit -m "feat(dashboard/ui): toml.js — smol-toml 双向转换 + round-trip 测试"
```

---

## Task 3: `configSchema.js` + `validate.js` — 字段元数据与校验

**Files:**
- Create: `crates/dashboard/ui/src/lib/configSchema.js`
- Create: `crates/dashboard/ui/src/lib/validate.js`
- Create: `crates/dashboard/ui/src/lib/validate.test.js`

- [ ] **Step 1: 写失败测试**

Create `crates/dashboard/ui/src/lib/validate.test.js`:

```js
import { test, expect } from "vitest";
import { validateModel } from "./validate.js";

test("empty model (all sections omitted) is valid", () => {
  expect(validateModel({})).toEqual([]);
});

test("valid model has no errors", () => {
  const m = {
    retrieval: { strategy: "bm25", top_k: 10 },
    upstream: [{ name: "mock", transport: "stdio", command: "/bin/x", call_timeout_ms: 30000 }],
  };
  expect(validateModel(m)).toEqual([]);
});

test("strategy out of enum", () => {
  const e = validateModel({ retrieval: { strategy: "bogus", top_k: 10 } });
  expect(e.some((x) => x.path === "retrieval.strategy")).toBe(true);
});

test("top_k must be >=1 integer", () => {
  const e = validateModel({ retrieval: { strategy: "bm25", top_k: 0 } });
  expect(e.some((x) => x.path === "retrieval.top_k")).toBe(true);
});

test("vector strategy requires vector.model + api_key_env", () => {
  const e = validateModel({ retrieval: { strategy: "vector", top_k: 5, vector: {} } });
  expect(e.some((x) => x.path === "retrieval.vector.model")).toBe(true);
  expect(e.some((x) => x.path === "retrieval.vector.api_key_env")).toBe(true);
});

test("upstream name cannot contain __ and must be unique", () => {
  const e = validateModel({ upstream: [
    { name: "a__b", transport: "stdio", command: "/x" },
    { name: "dup", transport: "stdio", command: "/x" },
    { name: "dup", transport: "stdio", command: "/x" },
  ]});
  expect(e.some((x) => x.path === "upstream[0].name" && /__/.test(x.msg))).toBe(true);
  expect(e.some((x) => x.path === "upstream[2].name" && /重复/.test(x.msg))).toBe(true);
});

test("stdio requires command, http requires url", () => {
  const e = validateModel({ upstream: [
    { name: "s", transport: "stdio" },
    { name: "h", transport: "http" },
  ]});
  expect(e.some((x) => x.path === "upstream[0].command")).toBe(true);
  expect(e.some((x) => x.path === "upstream[1].url")).toBe(true);
});

test("transport out of enum", () => {
  const e = validateModel({ upstream: [{ name: "x", transport: "grpc" }] });
  expect(e.some((x) => x.path === "upstream[0].transport")).toBe(true);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd crates/dashboard/ui && npm run test -- validate`
Expected: FAIL（`Failed to resolve import "./validate.js"`）。

- [ ] **Step 3: 实现 `configSchema.js` 与 `validate.js`**

Create `crates/dashboard/ui/src/lib/configSchema.js`:

```js
// Enumerations + section metadata shared by the form sections and the validator.
export const STRATEGIES = ["bm25", "vector", "subagent"];
export const TRANSPORTS = ["stdio", "http"];

// Left-nav order of top-level sections (TOML-native key names).
export const SECTIONS = ["retrieval", "server", "audit", "dashboard", "upstream"];

// Only `[[upstream]]` hot-reloads; everything else needs a restart.
export const HOT_RELOAD_SECTIONS = ["upstream"];
export function sectionReload(section) {
  return HOT_RELOAD_SECTIONS.includes(section) ? "hot" : "restart";
}
```

Create `crates/dashboard/ui/src/lib/validate.js`:

```js
import { STRATEGIES, TRANSPORTS } from "./configSchema.js";

/**
 * Field-level validation of a config model. Pure function, never throws.
 * Returns an array of { path, msg } (empty = valid). The BACKEND remains the
 * authority for env-resolution and full structural validation at Save time.
 */
export function validateModel(model) {
  const errors = [];
  const push = (path, msg) => errors.push({ path, msg });

  const r = model.retrieval;
  if (r) {
    if (r.strategy !== undefined && !STRATEGIES.includes(r.strategy))
      push("retrieval.strategy", `strategy 必须是 ${STRATEGIES.join(" / ")}`);
    if (r.top_k !== undefined && (!Number.isInteger(r.top_k) || r.top_k < 1))
      push("retrieval.top_k", "top_k 必须是 ≥1 的整数");
    if (r.strategy === "vector") requireSub(push, "retrieval.vector", r.vector);
    if (r.strategy === "subagent") requireSub(push, "retrieval.subagent", r.subagent);
  }

  const ups = model.upstream;
  if (Array.isArray(ups)) {
    const seen = new Set();
    ups.forEach((u, i) => {
      const base = `upstream[${i}]`;
      if (!u.name || !u.name.trim()) push(`${base}.name`, "name 必填");
      else {
        if (u.name.includes("__")) push(`${base}.name`, 'name 不能包含 "__"');
        if (seen.has(u.name)) push(`${base}.name`, `name "${u.name}" 重复`);
        seen.add(u.name);
      }
      if (u.call_timeout_ms !== undefined && (!Number.isInteger(u.call_timeout_ms) || u.call_timeout_ms < 1))
        push(`${base}.call_timeout_ms`, "call_timeout_ms 必须是 ≥1 的整数");
      if (!TRANSPORTS.includes(u.transport))
        push(`${base}.transport`, `transport 必须是 ${TRANSPORTS.join(" / ")}`);
      else if (u.transport === "stdio") {
        if (!u.command || !u.command.trim()) push(`${base}.command`, "stdio 上游 command 必填");
      } else if (u.transport === "http") {
        if (!u.url || !u.url.trim()) push(`${base}.url`, "http 上游 url 必填");
      }
    });
  }

  return errors;
}

function requireSub(push, path, sub) {
  if (!sub || !sub.model || !sub.model.trim()) push(`${path}.model`, "model 必填");
  if (!sub || !sub.api_key_env || !sub.api_key_env.trim()) push(`${path}.api_key_env`, "api_key_env 必填");
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd crates/dashboard/ui && npm run test -- validate`
Expected: PASS（8 passed）。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/ui/src/lib/configSchema.js crates/dashboard/ui/src/lib/validate.js crates/dashboard/ui/src/lib/validate.test.js
git commit -m "feat(dashboard/ui): configSchema + validate 字段级校验 + corner 测试"
```

---

> **关于 Svelte 组件类任务（Task 4–7）的验证**：本仓库前端不引入 Svelte 组件渲染测试（spec §8 决定只对纯函数 `toml.js`/`validate.js` 做 vitest）。因此组件任务的"测试"= **`npm run build` 编译通过** + **手动验证清单**（`npm run dev` 后在浏览器按清单逐项核对）。纯逻辑（同步、校验集成）已被 Task 2/3 的纯函数测试覆盖。

## Task 4: 抽出 `RawEditor` + `Config` 容器加 `Raw│Form` 切换骨架

**Files:**
- Create: `crates/dashboard/ui/src/lib/RawEditor.svelte`
- Modify: `crates/dashboard/ui/src/lib/Config.svelte`（整文件重写，见下）

- [ ] **Step 1: 抽出 `RawEditor.svelte`**

Create `crates/dashboard/ui/src/lib/RawEditor.svelte`:

```svelte
<script>
  // Raw TOML text editor. `content` is two-way bound to the parent.
  let { content = $bindable("") } = $props();
</script>

<textarea class="cfg-edit" bind:value={content} spellcheck="false" aria-label="config TOML"></textarea>
```

- [ ] **Step 2: 重写 `Config.svelte`（加 view 切换 + 用 RawEditor；Form 暂占位）**

Rewrite `crates/dashboard/ui/src/lib/Config.svelte`:

```svelte
<script>
  import { admin, adminGet, adminPut } from "./admin.svelte.js";
  import RawEditor from "./RawEditor.svelte";
  let content = $state("");
  let loaded = $state(false);
  let error = $state(null);
  let result = $state(null);
  let busy = $state(false);
  let reqId = 0;
  let view = $state("raw"); // "raw" | "form"

  async function load() {
    busy = true; error = null; result = null;
    const my = ++reqId;
    try {
      const r = await adminGet("/api/admin/config");
      if (my !== reqId) return; // superseded by a newer load
      if (r.status === 404) { error = "serve 未带 --config（无文件可改）"; loaded = false; return; }
      if (r.status === 401) { error = "admin token 失效，请在 About 重新输入"; loaded = false; return; }
      if (!r.ok) { error = `GET ${r.status}: ${await r.text()}`; loaded = false; return; }
      content = (await r.json()).content; loaded = true;
    } catch (e) { if (my === reqId) error = String(e); }
    finally { if (my === reqId) busy = false; }
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
    <div class="cfg-modes">
      <button class="admbtn" class:active={view === "raw"} onclick={() => (view = "raw")}>Raw</button>
      <button class="admbtn" class:active={view === "form"} onclick={() => (view = "form")}>Form</button>
    </div>
    {#if view === "raw"}
      <RawEditor bind:content />
    {:else}
      <p class="muted">表单模式将在 Task 5–7 接入。</p>
    {/if}
    <div class="toolbar">
      <button class="admbtn" onclick={save} disabled={busy}>{busy ? "saving…" : "Save"}</button>
      <button class="admbtn" onclick={load} disabled={busy}>Reload</button>
    </div>
    {#if result}
      <div class="card" style="margin-top:var(--s3)">
        <p>✓ saved · upstreams +{result.upstreams.added.length} −{result.upstreams.removed.length} ~{result.upstreams.reconnected.length}
          {#if result.upstreams.connect_failures.length}
            <span class="badge error" title={result.upstreams.connect_failures.map((f) => f[1]).join("; ")}>connect failed: {result.upstreams.connect_failures.map((f) => f[0]).join(", ")}</span>
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

- [ ] **Step 3: 编译确认**

Run: `cd crates/dashboard/ui && npm run build`
Expected: 构建成功、exit 0（Svelte 编译无错）。

- [ ] **Step 4: 手动验证清单**

`npm run dev` 后于浏览器（输入 admin token 解锁 Config）：
- Raw 模式：load 出当前 TOML、编辑、Save 触发结果卡片、Reload 复原 —— 与改造前一致。
- 顶部出现 `Raw │ Form` 两个按钮，点击可切换；Form 暂显示占位文字。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/ui/src/lib/RawEditor.svelte crates/dashboard/ui/src/lib/Config.svelte
git commit -m "refactor(dashboard/ui): 抽出 RawEditor + Config 加 Raw│Form 切换骨架"
```

---

## Task 5: `FormEditor` + 简单段组件（Retrieval/Server/Audit/Dashboard）

布局 A：左段导航 + 右段表单。单例段惰性创建（undefined → `[启用]`）。本任务创建组件文件并由 `FormEditor` 引用（含 `SectionUpstreams` 占位，Task 6 重写）；接入 `Config.svelte` 在 Task 7。

**Files:**
- Modify: `crates/dashboard/ui/src/lib/configSchema.js`（加 `defaultSection`）
- Create: `crates/dashboard/ui/src/lib/FormEditor.svelte`
- Create: `crates/dashboard/ui/src/lib/SectionRetrieval.svelte`
- Create: `crates/dashboard/ui/src/lib/SectionServer.svelte`
- Create: `crates/dashboard/ui/src/lib/SectionAudit.svelte`
- Create: `crates/dashboard/ui/src/lib/SectionDashboard.svelte`
- Create: `crates/dashboard/ui/src/lib/SectionUpstreams.svelte`（占位 stub）

- [ ] **Step 1: 扩展 `configSchema.js` 加 `defaultSection`**

追加到 `crates/dashboard/ui/src/lib/configSchema.js` 末尾：

```js
// Default value for a section when the user enables it from the form (aligned with the
// config crate's #[serde(default)] sensible defaults). `upstream` is handled as an array.
export function defaultSection(name) {
  switch (name) {
    case "retrieval": return { strategy: "bm25", top_k: 10 };
    case "server": return { stdio: false };
    case "audit": return { enabled: false, path: "mcpgw-audit.jsonl" };
    case "dashboard":
      return { enabled: false, bind: "127.0.0.1:8971", trace_queries: false,
               trace_buffer: 500, call_buffer: 500, payload_max_bytes: 4096 };
    default: return {};
  }
}
```

- [ ] **Step 2: `FormEditor.svelte`（左导航 + 右段）**

Create `crates/dashboard/ui/src/lib/FormEditor.svelte`:

```svelte
<script>
  import { SECTIONS, sectionReload } from "./configSchema.js";
  import SectionRetrieval from "./SectionRetrieval.svelte";
  import SectionServer from "./SectionServer.svelte";
  import SectionAudit from "./SectionAudit.svelte";
  import SectionDashboard from "./SectionDashboard.svelte";
  import SectionUpstreams from "./SectionUpstreams.svelte";

  let { model = $bindable() } = $props();
  let current = $state("retrieval");
  const LABELS = { retrieval: "Retrieval", server: "Server", audit: "Audit", dashboard: "Dashboard", upstream: "Upstreams" };
</script>

<div class="cfg-form">
  <nav class="cfg-nav">
    {#each SECTIONS as s}
      <button type="button" class="cfg-navitem" class:active={current === s} onclick={() => (current = s)}>
        <span>{LABELS[s]}</span>
        <span class="badge {sectionReload(s) === 'hot' ? 'ok' : 'skipped'}">{sectionReload(s) === 'hot' ? '🔥' : '⟳'}</span>
      </button>
    {/each}
  </nav>
  <div class="cfg-pane">
    {#if current === "retrieval"}<SectionRetrieval bind:retrieval={model.retrieval} />
    {:else if current === "server"}<SectionServer bind:server={model.server} />
    {:else if current === "audit"}<SectionAudit bind:audit={model.audit} />
    {:else if current === "dashboard"}<SectionDashboard bind:dashboard={model.dashboard} />
    {:else if current === "upstream"}<SectionUpstreams bind:upstream={model.upstream} />
    {/if}
  </div>
</div>
```

- [ ] **Step 3: `SectionRetrieval.svelte`**

Create `crates/dashboard/ui/src/lib/SectionRetrieval.svelte`:

```svelte
<script>
  import { STRATEGIES, defaultSection } from "./configSchema.js";
  let { retrieval = $bindable() } = $props();
  // Lazily create the vector/subagent sub-table when its strategy is selected.
  $effect(() => {
    if (retrieval?.strategy === "vector" && !retrieval.vector) retrieval.vector = { model: "", api_key_env: "" };
    if (retrieval?.strategy === "subagent" && !retrieval.subagent) retrieval.subagent = { model: "", api_key_env: "" };
  });
</script>

{#if retrieval === undefined}
  <p class="muted">[retrieval] 段未配置（运行时按默认值）。</p>
  <button type="button" class="admbtn" onclick={() => (retrieval = defaultSection("retrieval"))}>+ 启用 [retrieval]</button>
{:else}
  <label class="cfg-field">strategy
    <select bind:value={retrieval.strategy}>{#each STRATEGIES as s}<option value={s}>{s}</option>{/each}</select>
  </label>
  <label class="cfg-field">top_k <input type="number" min="1" bind:value={retrieval.top_k} /></label>

  {#if retrieval.strategy === "vector" && retrieval.vector}
    <fieldset class="cfg-sub"><legend>vector</legend>
      <label class="cfg-field">base_url <input bind:value={retrieval.vector.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">model <input bind:value={retrieval.vector.model} /></label>
      <label class="cfg-field">api_key_env <input bind:value={retrieval.vector.api_key_env} placeholder="环境变量名" /></label>
      <label class="cfg-field">dim <input type="number" min="1" bind:value={retrieval.vector.dim} /></label>
      <label class="cfg-field">timeout_ms <input type="number" min="1" bind:value={retrieval.vector.timeout_ms} /></label>
      <label class="cfg-field">batch_size <input type="number" min="1" bind:value={retrieval.vector.batch_size} /></label>
    </fieldset>
  {/if}
  {#if retrieval.strategy === "subagent" && retrieval.subagent}
    <fieldset class="cfg-sub"><legend>subagent</legend>
      <label class="cfg-field">base_url <input bind:value={retrieval.subagent.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">model <input bind:value={retrieval.subagent.model} /></label>
      <label class="cfg-field">api_key_env <input bind:value={retrieval.subagent.api_key_env} placeholder="环境变量名" /></label>
      <label class="cfg-field">timeout_ms <input type="number" min="1" bind:value={retrieval.subagent.timeout_ms} /></label>
      <label class="cfg-field">candidates <input type="number" min="1" bind:value={retrieval.subagent.candidates} /></label>
    </fieldset>
  {/if}
{/if}
```

- [ ] **Step 4: `SectionServer.svelte`**

Create `crates/dashboard/ui/src/lib/SectionServer.svelte`:

```svelte
<script>
  import { defaultSection } from "./configSchema.js";
  let { server = $bindable() } = $props();
  function enableHttp() { server.http = { enabled: false, bind: "127.0.0.1:8970", path: "/mcp", api_key: [] }; }
  function addKey() { server.http.api_key = [...(server.http.api_key ?? []), { name: "", env: "" }]; }
  function rmKey(i) { server.http.api_key = server.http.api_key.filter((_, j) => j !== i); }
</script>

{#if server === undefined}
  <button type="button" class="admbtn" onclick={() => (server = defaultSection("server"))}>+ 启用 [server]</button>
{:else}
  <label class="cfg-field cfg-switch">stdio <input type="checkbox" bind:checked={server.stdio} /></label>
  {#if !server.http}
    <button type="button" class="admbtn" onclick={enableHttp}>+ 启用 [server.http]</button>
  {:else}
    <fieldset class="cfg-sub"><legend>http</legend>
      <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={server.http.enabled} /></label>
      <label class="cfg-field">bind <input bind:value={server.http.bind} /></label>
      <label class="cfg-field">path <input bind:value={server.http.path} /></label>
      <div class="cfg-arr"><span class="label">api_key</span>
        {#each server.http.api_key ?? [] as k, i}
          <div class="cfg-arr-row">
            <input placeholder="name(标签)" bind:value={k.name} />
            <input placeholder="env(变量名)" bind:value={k.env} />
            <button type="button" class="admbtn" onclick={() => rmKey(i)}>✕</button>
          </div>
        {/each}
        <button type="button" class="admbtn" onclick={addKey}>+ add api_key</button>
      </div>
    </fieldset>
  {/if}
{/if}
```

- [ ] **Step 5: `SectionAudit.svelte` 与 `SectionDashboard.svelte`**

Create `crates/dashboard/ui/src/lib/SectionAudit.svelte`:

```svelte
<script>
  import { defaultSection } from "./configSchema.js";
  let { audit = $bindable() } = $props();
</script>

{#if audit === undefined}
  <button type="button" class="admbtn" onclick={() => (audit = defaultSection("audit"))}>+ 启用 [audit]</button>
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={audit.enabled} /></label>
  <label class="cfg-field">path <input bind:value={audit.path} /></label>
{/if}
```

Create `crates/dashboard/ui/src/lib/SectionDashboard.svelte`:

```svelte
<script>
  import { defaultSection } from "./configSchema.js";
  let { dashboard = $bindable() } = $props();
</script>

{#if dashboard === undefined}
  <button type="button" class="admbtn" onclick={() => (dashboard = defaultSection("dashboard"))}>+ 启用 [dashboard]</button>
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={dashboard.enabled} /></label>
  <label class="cfg-field">bind <input bind:value={dashboard.bind} /></label>
  <label class="cfg-field cfg-switch">trace_queries <input type="checkbox" bind:checked={dashboard.trace_queries} /></label>
  <label class="cfg-field">trace_path <input bind:value={dashboard.trace_path} placeholder="(可选)" /></label>
  <label class="cfg-field">trace_buffer <input type="number" min="0" bind:value={dashboard.trace_buffer} /></label>
  <label class="cfg-field">call_buffer <input type="number" min="0" bind:value={dashboard.call_buffer} /></label>
  <label class="cfg-field">payload_max_bytes <input type="number" min="0" bind:value={dashboard.payload_max_bytes} /></label>
  <label class="cfg-field">admin_token_env <input bind:value={dashboard.admin_token_env} placeholder="环境变量名(可选)" /></label>
  <label class="cfg-field">disabled_state_path <input bind:value={dashboard.disabled_state_path} placeholder="(可选)" /></label>
{/if}
```

- [ ] **Step 6: `SectionUpstreams.svelte` 占位 stub（Task 6 重写）**

Create `crates/dashboard/ui/src/lib/SectionUpstreams.svelte`:

```svelte
<script>
  let { upstream = $bindable() } = $props();
</script>

<p class="muted">upstream 数组编辑将在 Task 6 接入。</p>
```

- [ ] **Step 7: 编译确认**

Run: `cd crates/dashboard/ui && npm run build`
Expected: 构建成功、exit 0。

- [ ] **Step 8: Commit**

```bash
git add crates/dashboard/ui/src/lib/configSchema.js crates/dashboard/ui/src/lib/FormEditor.svelte crates/dashboard/ui/src/lib/SectionRetrieval.svelte crates/dashboard/ui/src/lib/SectionServer.svelte crates/dashboard/ui/src/lib/SectionAudit.svelte crates/dashboard/ui/src/lib/SectionDashboard.svelte crates/dashboard/ui/src/lib/SectionUpstreams.svelte
git commit -m "feat(dashboard/ui): FormEditor + retrieval/server/audit/dashboard 段表单"
```

---

## Task 6: `SectionUpstreams.svelte` — upstream 数组（增删 + transport 切换）

**Files:**
- Modify: `crates/dashboard/ui/src/lib/SectionUpstreams.svelte`（重写占位 stub）

- [ ] **Step 1: 重写 `SectionUpstreams.svelte`**

Replace `crates/dashboard/ui/src/lib/SectionUpstreams.svelte` with:

```svelte
<script>
  import { TRANSPORTS } from "./configSchema.js";
  let { upstream = $bindable() } = $props();

  function add() {
    upstream = [...(upstream ?? []), { name: "", transport: "stdio", command: "", call_timeout_ms: 30000 }];
  }
  function remove(i) { upstream = upstream.filter((_, j) => j !== i); }

  // Drop the other transport's fields on switch so serialized TOML stays clean.
  function onTransport(u) {
    if (u.transport === "stdio") {
      delete u.url; delete u.bearer_env; delete u.headers;
      if (u.command === undefined) u.command = "";
    } else {
      delete u.command; delete u.args; delete u.env_passthrough;
      if (u.url === undefined) u.url = "";
    }
  }

  function addHeader(u) { u.headers = { ...(u.headers ?? {}), "": "" }; }
  function setHeaderKey(u, oldK, newK) {
    const h = {}; for (const [k, v] of Object.entries(u.headers)) h[k === oldK ? newK : k] = v; u.headers = h;
  }
  function rmHeader(u, k) { const h = { ...u.headers }; delete h[k]; u.headers = h; }
</script>

{#if !upstream || upstream.length === 0}
  <p class="muted">无 upstream。</p>
{/if}
{#each upstream ?? [] as u, i}
  <fieldset class="cfg-sub cfg-upstream">
    <legend>upstream[{i}] <button type="button" class="admbtn" onclick={() => remove(i)}>✕ 移除</button></legend>
    <label class="cfg-field">name <input bind:value={u.name} placeholder="唯一、非空、不含 __" /></label>
    <label class="cfg-field">call_timeout_ms <input type="number" min="1" bind:value={u.call_timeout_ms} /></label>
    <label class="cfg-field">transport
      <select bind:value={u.transport} onchange={() => onTransport(u)}>
        {#each TRANSPORTS as t}<option value={t}>{t}</option>{/each}
      </select>
    </label>
    {#if u.transport === "stdio"}
      <label class="cfg-field">command <input bind:value={u.command} placeholder="可执行路径" /></label>
      <label class="cfg-field">args <input value={(u.args ?? []).join(" ")} oninput={(e) => (u.args = e.target.value.split(/\s+/).filter(Boolean))} placeholder="空格分隔" /></label>
      <label class="cfg-field">env_passthrough <input value={(u.env_passthrough ?? []).join(" ")} oninput={(e) => (u.env_passthrough = e.target.value.split(/\s+/).filter(Boolean))} placeholder="如 PATH HOME" /></label>
    {:else if u.transport === "http"}
      <label class="cfg-field">url <input bind:value={u.url} placeholder="https://…/mcp" /></label>
      <label class="cfg-field">bearer_env <input bind:value={u.bearer_env} placeholder="环境变量名(可选)" /></label>
      <div class="cfg-arr"><span class="label">headers (header名 → env名)</span>
        {#each Object.entries(u.headers ?? {}) as [k, v]}
          <div class="cfg-arr-row">
            <input value={k} onchange={(e) => setHeaderKey(u, k, e.target.value)} placeholder="header 名" />
            <input value={v} onchange={(e) => (u.headers[k] = e.target.value)} placeholder="env 变量名" />
            <button type="button" class="admbtn" onclick={() => rmHeader(u, k)}>✕</button>
          </div>
        {/each}
        <button type="button" class="admbtn" onclick={() => addHeader(u)}>+ add header</button>
      </div>
    {/if}
  </fieldset>
{/each}
<button type="button" class="admbtn" onclick={add}>+ add upstream</button>
```

> 注：`args`/`env_passthrough` 用空格分隔的简化输入；含空格的参数请用 raw 模式编辑。

- [ ] **Step 2: 编译确认**

Run: `cd crates/dashboard/ui && npm run build`
Expected: 构建成功、exit 0。

- [ ] **Step 3: Commit**

```bash
git add crates/dashboard/ui/src/lib/SectionUpstreams.svelte
git commit -m "feat(dashboard/ui): SectionUpstreams 数组增删 + transport/headers 编辑"
```

---

## Task 7: `Config.svelte` 完整接线（raw↔model 同步 + 校验 + Save + 解析失败）

**Files:**
- Modify: `crates/dashboard/ui/src/lib/toml.js`（加 `pruneModel`，stringify 前清洗 null/undefined）
- Modify: `crates/dashboard/ui/src/lib/toml.test.js`（加 prune 测试）
- Modify: `crates/dashboard/ui/src/lib/Config.svelte`（整文件重写，接入 model 同步）

- [ ] **Step 1: 写 `pruneModel` 失败测试**

追加到 `crates/dashboard/ui/src/lib/toml.test.js`:

```js
import { pruneModel } from "./toml.js";

test("pruneModel drops null/undefined so smol-toml can serialize", () => {
  const m = { retrieval: { strategy: "bm25", top_k: null }, upstream: [{ name: "x", transport: "stdio", command: "/x", url: undefined }] };
  const p = pruneModel(m);
  expect("top_k" in p.retrieval).toBe(false);
  expect("url" in p.upstream[0]).toBe(false);
  expect(p.upstream[0].name).toBe("x");
});

test("stringifyToml tolerates null fields via prune", () => {
  const out = stringifyToml({ retrieval: { strategy: "bm25", top_k: 10, vector: null } });
  expect(out).toContain("strategy");
  expect(out).not.toContain("vector");
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd crates/dashboard/ui && npm run test -- toml`
Expected: FAIL（`pruneModel is not a function`）。

- [ ] **Step 3: 改 `toml.js` 加 `pruneModel` 并在 `stringifyToml` 中调用**

Replace `crates/dashboard/ui/src/lib/toml.js` with:

```js
import { parse, stringify } from "smol-toml";

/** Parse raw TOML into a model. Returns { ok:true, model } or { ok:false, error } — never throws. */
export function parseToml(raw) {
  try {
    return { ok: true, model: parse(raw) };
  } catch (e) {
    return { ok: false, error: e?.message ?? String(e) };
  }
}

/** Deep-copy a model dropping null/undefined values so smol-toml can serialize cleanly. */
export function pruneModel(value) {
  if (Array.isArray(value)) return value.map(pruneModel);
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      if (v === null || v === undefined) continue;
      out[k] = pruneModel(v);
    }
    return out;
  }
  return value;
}

/** Serialize a model to canonical TOML (comments NOT preserved; null/undefined pruned). */
export function stringifyToml(model) {
  return stringify(pruneModel(model));
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd crates/dashboard/ui && npm run test`
Expected: PASS（toml + validate + smoke 全过）。

- [ ] **Step 5: 重写 `Config.svelte` 完整接线**

Replace `crates/dashboard/ui/src/lib/Config.svelte` with:

```svelte
<script>
  import { admin, adminGet, adminPut } from "./admin.svelte.js";
  import { parseToml, stringifyToml } from "./toml.js";
  import { validateModel } from "./validate.js";
  import RawEditor from "./RawEditor.svelte";
  import FormEditor from "./FormEditor.svelte";

  let content = $state("");      // raw TOML text (source of truth for the raw view)
  let model = $state(null);      // parsed model (source of truth for the form view)
  let loaded = $state(false);
  let error = $state(null);
  let result = $state(null);
  let busy = $state(false);
  let view = $state("raw");      // "raw" | "form"
  let parseError = $state(null); // set when switching to form fails to parse
  let reqId = 0;

  const errors = $derived(view === "form" && model ? validateModel(model) : []);

  async function load() {
    busy = true; error = null; result = null; parseError = null;
    const my = ++reqId;
    try {
      const r = await adminGet("/api/admin/config");
      if (my !== reqId) return;
      if (r.status === 404) { error = "serve 未带 --config（无文件可改）"; loaded = false; return; }
      if (r.status === 401) { error = "admin token 失效，请在 About 重新输入"; loaded = false; return; }
      if (!r.ok) { error = `GET ${r.status}: ${await r.text()}`; loaded = false; return; }
      content = (await r.json()).content; loaded = true; view = "raw"; model = null;
    } catch (e) { if (my === reqId) error = String(e); }
    finally { if (my === reqId) busy = false; }
  }

  function toForm() {
    const r = parseToml(content);
    if (!r.ok) { parseError = r.error; view = "form"; model = null; return; }
    parseError = null; model = r.model; view = "form";
  }
  function toRaw() {
    if (model) content = stringifyToml(model);
    view = "raw";
  }

  async function save() {
    busy = true; error = null; result = null;
    try {
      const body = view === "form" && model ? stringifyToml(model) : content;
      content = body; // keep raw in sync with what we sent
      const r = await adminPut("/api/admin/config", body);
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
    <div class="cfg-modes">
      <button class="admbtn" class:active={view === "raw"} onclick={toRaw}>Raw</button>
      <button class="admbtn" class:active={view === "form"} onclick={toForm}>Form</button>
    </div>

    {#if view === "raw"}
      <RawEditor bind:content />
    {:else if parseError}
      <p class="error" role="alert">raw 有语法错误，修正后可结构化编辑：{parseError}</p>
    {:else if model}
      <FormEditor bind:model />
      {#if errors.length}
        <ul class="cfg-errs">{#each errors as e}<li><code>{e.path}</code> — {e.msg}</li>{/each}</ul>
      {/if}
    {/if}

    <div class="toolbar">
      <button class="admbtn" onclick={save} disabled={busy || (view === "form" && errors.length > 0)}>{busy ? "saving…" : "Save"}</button>
      <button class="admbtn" onclick={load} disabled={busy}>Reload</button>
    </div>
    {#if result}
      <div class="card" style="margin-top:var(--s3)">
        <p>✓ saved · upstreams +{result.upstreams.added.length} −{result.upstreams.removed.length} ~{result.upstreams.reconnected.length}
          {#if result.upstreams.connect_failures.length}
            <span class="badge error" title={result.upstreams.connect_failures.map((f) => f[1]).join("; ")}>connect failed: {result.upstreams.connect_failures.map((f) => f[0]).join(", ")}</span>
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

- [ ] **Step 6: 编译 + 全部测试**

Run: `cd crates/dashboard/ui && npm run build && npm run test`
Expected: build exit 0；test 全过。

- [ ] **Step 7: 手动验证清单**（`npm run dev` + admin token）

- Raw→Form：合法 raw 切 Form，各段字段正确回填；切回 Raw，内容规范化、语义不变。
- Form 编辑：改 top_k / strategy 切换（vector/subagent 子表出现）/ server.http / upstream 增删与 stdio↔http 切换 / headers 增删。
- 校验：把 upstream name 清空或填 `a__b`，下方出现错误项且 `Save` 禁用；修正后恢复。
- Save（Form）：序列化 model→PUT，结果卡片显示 reconcile + needs_restart（改 upstream 热生效、改其它段 needs_restart）。
- 解析失败：raw 写错 TOML 再切 Form，显示"raw 有语法错误…"。

- [ ] **Step 8: Commit**

```bash
git add crates/dashboard/ui/src/lib/toml.js crates/dashboard/ui/src/lib/toml.test.js crates/dashboard/ui/src/lib/Config.svelte
git commit -m "feat(dashboard/ui): Config 接入 raw↔model 同步 + 校验 + Save + pruneModel"
```

---

## Task 8: 表单样式（`app.css`）+ 重建 `dist`

类名全局唯一（`app.css` 非 scoped），统一用 `.cfg-` 前缀；复用既有 token。

**Files:**
- Modify: `crates/dashboard/ui/src/app.css`（追加 `.cfg-*` 样式）
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: 追加表单样式到 `app.css`**

在 `crates/dashboard/ui/src/app.css` 的 `.cfg-edit { … }` 规则之后追加：

```css
/* ---- Config form mode ---- */
.cfg-modes { display: flex; gap: var(--s2); margin-bottom: var(--s3); }
.cfg-modes .admbtn.active { color: var(--fg); border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }

.cfg-form { display: flex; gap: var(--s4); align-items: flex-start; }
.cfg-nav { display: flex; flex-direction: column; gap: var(--s1); min-width: 150px; }
.cfg-navitem { display: flex; align-items: center; justify-content: space-between; gap: var(--s2);
  padding: var(--s2) var(--s3); border: 1px solid var(--border); border-radius: var(--r-sm);
  background: var(--panel); color: var(--fg-dim); cursor: pointer; font: inherit; text-align: left; }
.cfg-navitem:hover { border-color: var(--border-hover); color: var(--fg); }
.cfg-navitem.active { border-color: var(--accent); color: var(--fg); background: var(--accent-soft); }
.cfg-pane { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: var(--s3); }

.cfg-field { display: flex; flex-direction: column; gap: var(--s1); font-size: var(--fs-sm); color: var(--fg-dim); }
.cfg-field input, .cfg-field select { background: var(--raised); border: 1px solid var(--border);
  border-radius: var(--r-sm); color: var(--fg); padding: var(--s2); font: inherit; }
.cfg-field input:focus, .cfg-field select:focus { outline: none; border-color: var(--accent); box-shadow: var(--ring); }
.cfg-switch { flex-direction: row; align-items: center; gap: var(--s2); }
.cfg-switch input { width: auto; }

.cfg-sub { border: 1px solid var(--border); border-radius: var(--r-md); padding: var(--s3);
  display: flex; flex-direction: column; gap: var(--s2); }
.cfg-sub legend { color: var(--muted); font-size: var(--fs-xs); padding: 0 var(--s1); }
.cfg-upstream legend { display: flex; align-items: center; gap: var(--s2); }

.cfg-arr { display: flex; flex-direction: column; gap: var(--s2); }
.cfg-arr-row { display: flex; gap: var(--s2); align-items: center; }
.cfg-arr-row input { flex: 1; }

.cfg-errs { list-style: none; padding: var(--s2) var(--s3); margin: var(--s2) 0 0;
  border: 1px solid var(--danger-bd); background: var(--danger-bg); border-radius: var(--r-sm);
  color: var(--danger); font-size: var(--fs-sm); display: flex; flex-direction: column; gap: var(--s1); }
.cfg-errs code { color: var(--fg); }
```

- [ ] **Step 2: 重建并提交 `dist`**

Run: `cd crates/dashboard/ui && npm run build`
Expected: 生成新的 `dist/assets/index-*.js` + `index-*.css`（hash 变化）、`dist/index.html` 更新。

- [ ] **Step 3: 手动验证样式**

`npm run dev`：左段导航高亮、字段/下拉/开关、子表 fieldset、upstream 增删行、校验错误列表样式与面板整体一致（深色、accent 高亮）。

- [ ] **Step 4: Commit**

```bash
git add crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "style(dashboard/ui): 配置表单样式 + 重建 dist"
```

---

## Task 9: 文档同步（L1–L4）

后端契约未变，主要更新 dashboard 组件相关层。

**Files:**
- Modify: `docs/L1-overview.md`
- Modify: `docs/L2-components/dashboard.md`
- Modify: `docs/L3-details/dashboard.md`

- [ ] **Step 1: 更新文档**

按以下要点补充（保持各文件既有中文风格）：

- `docs/L1-overview.md`：写子系统 C 处补一句——Config 提供 **raw（保真）+ 结构化表单（全段、即时校验）** 两种模式，表单经前端 smol-toml 与 raw 实时双向同步，保存复用同一 `PUT /api/admin/config`（后端零改动）。
- `docs/L2-components/dashboard.md`：在 Config 相关条目补：
  - 两视图 `Raw │ Form`；前端新增组件 `RawEditor` / `FormEditor` / `SectionRetrieval|Server|Audit|Dashboard|Upstreams` + 纯函数 `lib/toml.js`（smol-toml 双向）/ `lib/validate.js`（字段级校验）/ `lib/configSchema.js`（枚举/默认/段标注）。
  - model 用 **TOML 原始键名**（`upstream`/`api_key`、transport flatten）；表单模式保存即规范化（丢注释）。
  - 后端 `admin_config.rs` / API **不变**。
- `docs/L3-details/dashboard.md`：补表单模式机制：
  - 同步：切 Form `parseToml(content)→model`（失败→禁用+提示）；切 Raw / Save `stringifyToml(model)→content`（经 `pruneModel` 删 null/undefined）。
  - 校验两层：前端 `validateModel`（必填/枚举/number/`name` 规则/唯一）即时、有错禁用 Save；后端 Save 时权威校验 env 引用可解析。
  - 段惰性创建（undefined → `[启用]`）；`upstream` 🔥热生效、其余段 ⟳需重启（导航标注）。
  - 测试：vitest 覆盖 `toml.js` round-trip 与 `validate.js` corner cases。

- [ ] **Step 2: Commit**

```bash
git add docs/L1-overview.md docs/L2-components/dashboard.md docs/L3-details/dashboard.md
git commit -m "docs: L1–L3 同步 Config 结构化表单模式"
```

---

## Task 10: 最终验收

- [ ] **Step 1（可选）: Rust 契约测试锚定 schema**

在 `crates/config/src/lib.rs` 的 `#[cfg(test)]` 加一个测试：用一段代表性的"前端会生成的规范化 TOML"（含 `[[upstream]]` stdio+http、`[server.http]` + `[[server.http.api_key]]`、`[retrieval.vector]`）跑 `Config::from_toml_str(...).unwrap()`，断言关键字段（`upstreams.len()`、transport、`api_keys` 等）。捕获"前端 stringify 的 TOML 后端不认"的漂移。

```rust
#[test]
fn frontend_normalized_toml_is_accepted() {
    let toml = r#"
[retrieval]
strategy = "vector"
top_k = 10
[retrieval.vector]
model = "e5"
api_key_env = "VK"
[server]
stdio = false
[server.http]
enabled = true
bind = "127.0.0.1:8970"
path = "/mcp"
[[server.http.api_key]]
name = "a"
env = "K"
[[upstream]]
name = "mock"
transport = "stdio"
command = "/bin/mock"
[[upstream]]
name = "remote"
transport = "http"
url = "https://x/mcp"
"#;
    let c = config::Config::from_toml_str(toml).unwrap();
    assert_eq!(c.upstreams.len(), 2);
    assert_eq!(c.server.http.as_ref().unwrap().api_keys.len(), 1);
}
```
Run: `cargo test -p config frontend_normalized_toml_is_accepted` → PASS。Commit: `git commit -am "test(config): 锚定前端规范化 TOML 契约"`。

- [ ] **Step 2: 后端零回归**

Run: `cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features && cargo build --locked`
Expected: 全绿（后端未改产品代码）。

- [ ] **Step 3: 前端测试 + dist 可复现**

Run: `cd crates/dashboard/ui && npm run test && rm -rf dist && npm ci && npm run build && cd ../../.. && git status --porcelain crates/dashboard/ui/dist`
Expected: vitest 全过；`git status` 对 `dist/` 输出为空（committed dist 字节级可复现）。

- [ ] **Step 4: demo 手测**

重建后重启 demo（`MCPGW_DASH_ADMIN=… ./target/debug/mcpgw --config mcpgw.toml serve`），浏览器走一遍 Task 7 Step 7 的手动验证清单。

---

## 完成标准

- 9（+1 可选）个 Task 全部提交；前端 `npm run test` 全过、`dist` 字节级可复现；后端四道闸门零回归；demo 端到端验证两视图同步 + 表单编辑 + 热重载。
