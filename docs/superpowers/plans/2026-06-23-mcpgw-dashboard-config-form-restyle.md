# Dashboard Config 表单视觉重设计 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 dashboard Config 表单从功能性朴素样式重设计为对齐 dashboard 深色设计语言的精致界面（默认 Form、容器化、精致左导航、segmented 切换、sub-panel 段、字段统一、数组行卡片化、微交互），逻辑零改动。

**Architecture:** 纯前端视觉改动，全部在 `crates/dashboard/ui`。重写 `app.css` 的 `.cfg-*` 规则（复用现有 token `--r-lg/md/sm`/`--panel/panel-2`/`--accent`/`--brand-fill`/`--shadow-sm`/`--hover`/`--ring`/`.iconbtn`/sidebar-active 语言）+ 组件结构微调（默认 `view="form"`、容器 wrapper、`<fieldset><legend>`→`<div class="cfg-sub"><div class="cfg-sub-h">`、数组按钮→`.iconbtn`）。校验/同步/Save/后端**完全不变**——现有 28 vitest 必须保持全绿（回归保护）。

**Tech Stack:** Svelte 5（runes）、Vite、全局 `app.css`（非 scoped，`.cfg-` 前缀类名全局唯一）。

---

## 视觉任务的验证方式（贯穿所有 task）

无新单测（逻辑零改动）。每个 task 的"测试" = ① `npm run test` 仍 **28 passed**（回归：逻辑未碰）；② `npm run build` exit 0；③ 重建并提交 `dist/`；④ **视觉手测**（`npm run dev` 或合并后 demo，按该 task 的清单逐项核对深色下与 dashboard 其他页面观感一致）。组件改动只动包裹元素/class，**绝不改** `bind:`/`onclick`/`onchange`/`oninput`/`$effect`/`$derived`/函数体。

---

## File Structure

全部在 `crates/dashboard/ui/`：

| 文件 | 改动 | Task |
| --- | --- | --- |
| `src/lib/Config.svelte` | 默认 `view="form"`；外层 `.cfg-panel` 容器 + 顶部 `.cfg-bar`（segmented 切换 + Save/Reload）+ `.cfg-body` | 1 |
| `src/app.css` `.cfg-*` | 重写：①容器 `.cfg-panel/.cfg-bar/.cfg-seg/.cfg-body` | 1 |
| `src/lib/FormEditor.svelte` | 导航/pane class（结构基本不变，靠 CSS） | 2 |
| `src/lib/SectionRetrieval.svelte` | `<fieldset class="cfg-sub"><legend>` → `<div class="cfg-sub"><div class="cfg-sub-h">` (vector/subagent) | 2 |
| `src/lib/SectionServer.svelte` | 同上（http 子表）+ api_key 数组按钮→`.iconbtn` | 2,3 |
| `src/lib/SectionUpstreams.svelte` | 同上（upstream 子表，legend→sub-h 含 remove）+ 数组/headers 按钮→`.iconbtn` | 2,3 |
| `src/app.css` `.cfg-*` | ②导航 `.cfg-nav/.cfg-navitem`(sidebar 风格) + `.cfg-sub/.cfg-sub-h` | 2 |
| `src/lib/SectionAudit/Dashboard.svelte` | 字段 class 微调（switch 等，多数靠 CSS） | 3 |
| `src/app.css` `.cfg-*` | ③字段 `.cfg-field` + 数组 `.cfg-arr/.cfg-arr-row` + 错误 `.cfg-errs` + 微交互 | 3 |
| `src/lib/dist/` | 每个改动 task 重建并提交 | 1–3 |

---

## Task 1: 默认 Form + 容器化 panel + segmented 切换

把 Config 套进圆角 panel（顶部工具条 + body），`Raw│Form` 改 segmented pill，默认进 Form。

**Files:**
- Modify: `crates/dashboard/ui/src/lib/Config.svelte`
- Modify: `crates/dashboard/ui/src/app.css`
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: `Config.svelte` 默认 Form**

把 `<script>` 里的 `let view = $state("raw");` 改为：
```js
  let view = $state("form"); // default to the structured form
```
（其余 `<script>` 不变：`content`/`model`/`loaded`/`error`/`result`/`busy`/`parseError`/`reqId`、`errors` derived、`load`/`toForm`/`toRaw`/`save`/`$effect` 全部保持。注意 `toForm` 在首次 `$effect`→`load()` 后由用户点击触发；默认 `view="form"` 时初始 `model` 仍为 null，但 `load()` 设 `view="raw"`——见 Step 2 的处理。）

- [ ] **Step 2: `Config.svelte` 模板重写为 panel 容器**

`load()` 末尾当前是 `content = (await r.json()).content; loaded = true; view = "raw"; model = null;`。改为拉取后调用**现有** `toForm()` 进入 Form（复用现成函数，不新增同步逻辑）：
```js
      content = (await r.json()).content; loaded = true;
      toForm(); // default to Form: parse content→model（失败则 parseError，用户点 Raw 修）
```
（`toForm()` 已在 `<script>` 定义：parse 成功→设 `model`+`view="form"`；失败→设 `parseError`+`view="form"`。）Then replace the template block from `{#if !admin.token}` to the end of the file with:
```svelte
{#if !admin.token}
  <p class="muted">需要 admin token（在 About 页输入）才能编辑配置。</p>
{:else}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if loaded}
    <div class="cfg-panel">
      <div class="cfg-bar">
        <div class="cfg-seg" role="tablist">
          <button type="button" role="tab" class:active={view === "raw"} aria-selected={view === "raw"} onclick={toRaw}>Raw</button>
          <button type="button" role="tab" class:active={view === "form"} aria-selected={view === "form"} onclick={toForm}>Form</button>
        </div>
        <div class="cfg-actions">
          <button class="admbtn" onclick={save} disabled={busy || (view === "form" && (parseError || errors.length > 0))}>{busy ? "saving…" : "Save"}</button>
          <button class="admbtn" onclick={load} disabled={busy}>Reload</button>
        </div>
      </div>
      <div class="cfg-body">
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
      </div>
    </div>
    {#if result}
      <div class="card" style="margin-top:var(--s4)">
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
（这把原来分散的 `.cfg-modes` 切换 + 底部 `.toolbar` Save/Reload 合并到顶部 `.cfg-bar`，内容移入 `.cfg-body`。`parseToml` 已在 `<script>` import，可直接用于 Step 2 的 load。）

- [ ] **Step 3: `app.css` 加容器/工具条/segmented 样式**

把 `app.css` 中现有的 `.cfg-modes { … }` 与 `.cfg-modes .admbtn.active { … }` 两行（在 `/* ---- Config form mode ---- */` 下）替换为：
```css
/* ---- Config form mode: container ---- */
.cfg-panel { border: 1px solid var(--border); border-radius: var(--r-lg); background: var(--panel);
  box-shadow: var(--shadow-sm); overflow: hidden; }
.cfg-bar { display: flex; align-items: center; justify-content: space-between; gap: var(--s3);
  padding: var(--s3) var(--s4); border-bottom: 1px solid var(--border); background: var(--panel-2); }
.cfg-bar .cfg-actions { display: flex; gap: var(--s2); }
.cfg-body { padding: var(--s4); }

.cfg-seg { display: inline-flex; gap: 2px; padding: 3px; border: 1px solid var(--border);
  border-radius: var(--r-pill); background: var(--panel); }
.cfg-seg button { font: inherit; font-size: var(--fs-sm); cursor: pointer; border: 0; background: none;
  color: var(--muted); padding: 4px 16px; border-radius: var(--r-pill);
  transition: color .14s, background .14s; }
.cfg-seg button:hover { color: var(--fg); }
.cfg-seg button.active { background: var(--brand-fill); color: #fff; }
```

- [ ] **Step 4: build + 回归测试**

Run: `cd crates/dashboard/ui && npm run test` → expect **28 passed**（逻辑未改）.
Run: `npm run build` → exit 0.

- [ ] **Step 5: 视觉手测**（`npm run dev` + admin token）

- 打开 Config **默认进 Form**（不再是 raw）。
- 整体在一个圆角 panel 内：顶部工具条左侧是 `Raw│Form` 的 pill segmented（active 段填充 brand 蓝），右侧 Save/Reload；内容在 panel body。
- 点 Raw/Form 切换正常；Save/Reload/校验禁用/结果卡片功能不变。

- [ ] **Step 6: Commit（含重建 dist）**

```bash
git add crates/dashboard/ui/src/lib/Config.svelte crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "style(dashboard/ui): Config 默认 Form + 容器化 panel + segmented 切换"
```

---

## Task 2: 左导航精致化（sidebar 风格）+ 段 sub-panel（fieldset→div）

把原生 `<fieldset><legend>`（深色下割裂最突兀）换成柔和 sub-panel；左导航复用 sidebar 的渐变 active + accent 左条；段内/数组的次要按钮（启用/✕/＋）统一 `.iconbtn`（Save/Reload 保持 `.admbtn`）。**只换外壳元素/class，所有 `bind:`/`onclick`/`onchange`/`oninput`/函数体不变。**

**Files:**
- Modify: `crates/dashboard/ui/src/lib/SectionRetrieval.svelte`
- Modify: `crates/dashboard/ui/src/lib/SectionServer.svelte`
- Modify: `crates/dashboard/ui/src/lib/SectionUpstreams.svelte`
- Modify: `crates/dashboard/ui/src/lib/SectionAudit.svelte`
- Modify: `crates/dashboard/ui/src/lib/SectionDashboard.svelte`
- Modify: `crates/dashboard/ui/src/app.css`
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: `SectionRetrieval.svelte` — enable 按钮→iconbtn，vector/subagent fieldset→div**

Replace the enable button (the `+ 启用 [retrieval]` line) `class="admbtn"` → `class="iconbtn"`. Then replace the two `<fieldset class="cfg-sub"><legend>…</legend> … </fieldset>` blocks with `<div>` + `<div class="cfg-sub-h">`:
```svelte
  {#if (retrieval.strategy === "vector" || retrieval.strategy === "hybrid") && retrieval.vector}
    <div class="cfg-sub">
      <div class="cfg-sub-h">vector</div>
      <label class="cfg-field">base_url <input bind:value={retrieval.vector.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">model <input bind:value={retrieval.vector.model} /></label>
      <label class="cfg-field">api_key_env <input bind:value={retrieval.vector.api_key_env} placeholder="环境变量名" /></label>
      <label class="cfg-field">dim <input type="number" min="1" bind:value={retrieval.vector.dim} /></label>
      <label class="cfg-field">timeout_ms <input type="number" min="1" bind:value={retrieval.vector.timeout_ms} /></label>
      <label class="cfg-field">batch_size <input type="number" min="1" bind:value={retrieval.vector.batch_size} /></label>
    </div>
  {/if}
  {#if retrieval.strategy === "subagent" && retrieval.subagent}
    <div class="cfg-sub">
      <div class="cfg-sub-h">subagent</div>
      <label class="cfg-field">base_url <input bind:value={retrieval.subagent.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">model <input bind:value={retrieval.subagent.model} /></label>
      <label class="cfg-field">api_key_env <input bind:value={retrieval.subagent.api_key_env} placeholder="环境变量名" /></label>
      <label class="cfg-field">timeout_ms <input type="number" min="1" bind:value={retrieval.subagent.timeout_ms} /></label>
      <label class="cfg-field">candidates <input type="number" min="1" bind:value={retrieval.subagent.candidates} /></label>
    </div>
  {/if}
```

- [ ] **Step 2: `SectionServer.svelte` — enable 按钮→iconbtn，http fieldset→div，api_key 按钮→iconbtn**

Change both `+ 启用 [server]` / `+ 启用 [server.http]` buttons `class="admbtn"` → `class="iconbtn"`. Replace the http fieldset block with:
```svelte
    <div class="cfg-sub">
      <div class="cfg-sub-h">http</div>
      <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={server.http.enabled} /></label>
      <label class="cfg-field">bind <input bind:value={server.http.bind} /></label>
      <label class="cfg-field">path <input bind:value={server.http.path} /></label>
      <div class="cfg-arr"><span class="label">api_key</span>
        {#each server.http.api_key ?? [] as k, i}
          <div class="cfg-arr-row">
            <input placeholder="name(标签)" bind:value={k.name} />
            <input placeholder="env(变量名)" bind:value={k.env} />
            <button type="button" class="iconbtn" onclick={() => rmKey(i)}>✕</button>
          </div>
        {/each}
        <button type="button" class="iconbtn" onclick={addKey}>+ add api_key</button>
      </div>
    </div>
```

- [ ] **Step 3: `SectionUpstreams.svelte` — upstream fieldset→div（legend→sub-h 含 remove），数组/headers 按钮→iconbtn**

Replace the `{#each upstream …}` body's `<fieldset class="cfg-sub cfg-upstream"> … </fieldset>` with:
```svelte
  <div class="cfg-sub cfg-upstream">
    <div class="cfg-sub-h">upstream[{i}] <button type="button" class="iconbtn" onclick={() => remove(i)}>✕ 移除</button></div>
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
            <button type="button" class="iconbtn" onclick={() => rmHeader(u, k)}>✕</button>
          </div>
        {/each}
        <button type="button" class="iconbtn" onclick={() => addHeader(u)}>+ add header</button>
      </div>
    {/if}
  </div>
```
And change the final `+ add upstream` button `class="admbtn"` → `class="iconbtn"`.

- [ ] **Step 4: `SectionAudit.svelte` / `SectionDashboard.svelte` — enable 按钮→iconbtn**

In each, change the `+ 启用 [audit]` / `+ 启用 [dashboard]` button `class="admbtn"` → `class="iconbtn"`. (No fieldset in these two; fields stay as-is, restyled by CSS in Task 3.)

- [ ] **Step 5: `app.css` — 左导航 sidebar 风格 + sub-panel**

Replace the current `.cfg-form { … }` … `.cfg-pane { … }` block (the nav/pane rules) AND the `.cfg-sub { … }` + `.cfg-sub legend { … }` + `.cfg-upstream legend { … }` rules with:
```css
.cfg-form { display: flex; gap: var(--s4); align-items: flex-start; }
.cfg-nav { display: flex; flex-direction: column; gap: 2px; min-width: 156px; }
.cfg-navitem { position: relative; display: flex; align-items: center; justify-content: space-between;
  gap: var(--s2); padding: var(--s2) var(--s3); border: 0; border-radius: var(--r-sm);
  background: none; color: var(--fg-dim); cursor: pointer; font: inherit; font-size: var(--fs-sm);
  text-align: left; transition: background .14s, color .14s; }
.cfg-navitem:hover { background: var(--hover); color: var(--fg); }
.cfg-navitem.active { background: linear-gradient(90deg, rgba(91,157,255,.16), rgba(91,157,255,.04)); color: var(--fg); }
.cfg-navitem.active::before { content: ""; position: absolute; left: -3px; top: 6px; bottom: 6px;
  width: 3px; border-radius: 3px; background: var(--accent); }
.cfg-pane { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: var(--s4); }

.cfg-sub { border: 1px solid var(--border); border-radius: var(--r-md);
  padding: var(--s3) var(--s4) var(--s4); background: var(--panel-2);
  display: flex; flex-direction: column; gap: var(--s3); }
.cfg-sub-h { font-size: var(--fs-2xs); text-transform: uppercase; letter-spacing: .06em;
  color: var(--muted); display: flex; align-items: center; justify-content: space-between; gap: var(--s2);
  padding-bottom: var(--s2); border-bottom: 1px solid var(--border-soft); }
```

- [ ] **Step 6: build + 回归测试**

Run: `cd crates/dashboard/ui && npm run test` → **28 passed**. Run: `npm run build` → exit 0.

- [ ] **Step 7: 视觉手测**

左段导航：active 段有蓝色渐变背景 + accent 左条、hover 微亮，与主 sidebar 同语言但轻量；各子表（vector/http/upstream[i]）是柔和圆角 sub-panel（`--panel-2` 背景 + 小标题），不再有原生 fieldset 割裂；段内 ✕/＋/启用 是 iconbtn 风格；切换段、增删、校验、Save 功能不变。

- [ ] **Step 8: Commit（含重建 dist）**

```bash
git add crates/dashboard/ui/src/lib/SectionRetrieval.svelte crates/dashboard/ui/src/lib/SectionServer.svelte crates/dashboard/ui/src/lib/SectionUpstreams.svelte crates/dashboard/ui/src/lib/SectionAudit.svelte crates/dashboard/ui/src/lib/SectionDashboard.svelte crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "style(dashboard/ui): 左导航 sidebar 风格 + 段 sub-panel（fieldset→div）+ iconbtn"
```

---

## Task 3: 字段统一 + 数组行卡片化 + 错误卡片 + 微交互（纯 app.css）

组件已在 Task 2 改完 class，本 task 只精致化 `app.css`：input/select 统一背景层次/hover/focus ring、数值 tabular-nums；数组行卡片化（之前数组内 input 无独立样式，修复为统一控件）；错误列表柔和卡片；统一 `.14s` 过渡。

**Files:**
- Modify: `crates/dashboard/ui/src/app.css`
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: `app.css` — 替换字段/数组/错误规则**

Replace the current `.cfg-field { … }` / `.cfg-field input,select … ` / `:focus` / `.cfg-switch` rules AND the `.cfg-arr { … }` / `.cfg-arr-row { … }` / `.cfg-arr-row input { … }` rules AND the `.cfg-errs { … }` / `.cfg-errs code { … }` rules with:
```css
.cfg-field { display: flex; flex-direction: column; gap: 5px; font-size: var(--fs-sm); color: var(--fg-dim); }
.cfg-field > input, .cfg-field > select { background: var(--panel); border: 1px solid var(--border);
  border-radius: var(--r-sm); color: var(--fg); padding: 7px 10px; font: inherit;
  transition: border-color .14s, box-shadow .14s; }
.cfg-field > input[type="number"] { font-variant-numeric: tabular-nums; }
.cfg-field > input:hover, .cfg-field > select:hover { border-color: var(--border-hover); }
.cfg-field > input:focus, .cfg-field > select:focus { outline: none; border-color: var(--accent); box-shadow: var(--ring); }
.cfg-field.cfg-switch { flex-direction: row; align-items: center; gap: var(--s2); }
.cfg-field.cfg-switch input { width: auto; }

.cfg-arr { display: flex; flex-direction: column; gap: var(--s2); }
.cfg-arr .label { font-size: var(--fs-2xs); text-transform: uppercase; letter-spacing: .06em; color: var(--muted); }
.cfg-arr-row { display: flex; gap: var(--s2); align-items: center; padding: var(--s2);
  border: 1px solid var(--border); border-radius: var(--r-sm); background: var(--panel); }
.cfg-arr-row input { flex: 1; min-width: 0; background: var(--panel-2); border: 1px solid var(--border);
  border-radius: var(--r-sm); color: var(--fg); padding: 6px 9px; font: inherit;
  transition: border-color .14s, box-shadow .14s; }
.cfg-arr-row input:hover { border-color: var(--border-hover); }
.cfg-arr-row input:focus { outline: none; border-color: var(--accent); box-shadow: var(--ring); }

.cfg-errs { list-style: none; padding: var(--s3); margin: var(--s3) 0 0;
  border: 1px solid var(--danger-bd); background: var(--danger-bg); border-radius: var(--r-md);
  color: var(--danger); font-size: var(--fs-sm); display: flex; flex-direction: column; gap: var(--s1); }
.cfg-errs code { color: var(--fg); font-family: var(--mono); }
```

- [ ] **Step 2: build + 回归测试**

Run: `cd crates/dashboard/ui && npm run test` → **28 passed**. Run: `npm run build` → exit 0.

- [ ] **Step 3: 视觉手测**

字段 input/select：`--panel` 背景、圆角、hover border 变亮、focus 有 accent ring；数值右对齐等宽（tabular-nums）；switch 行水平对齐。数组行（api_key/headers）：每行是 `--panel` 卡片、内嵌 `--panel-2` input、统一圆角与 focus。错误列表：柔和 danger 圆角卡片，`<code>` 路径用 mono。整体在深色下与 dashboard 其他页面观感一致、无突兀。

- [ ] **Step 4: Commit（含重建 dist）**

```bash
git add crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git commit -m "style(dashboard/ui): 字段/数组行/错误卡片精致化 + 微交互"
```

---

## Task 4: 最终验收

- [ ] **Step 1: 前端回归 + dist 可复现**

```
cd crates/dashboard/ui && npm run test          # expect 28 passed (逻辑零改动)
rm -rf dist && npm ci && npm run build          # clean rebuild
cd ../../.. && git status --porcelain crates/dashboard/ui/dist   # MUST be empty (byte-reproducible)
```
若 `dist` 非空 → 报告漂移。

- [ ] **Step 2: 后端不受影响**

```
cargo build --locked    # rust-embed 嵌入新 dist 编译通过
cargo test --all-features   # 仍 328 passed（前端-only 改动）
```

- [ ] **Step 3: demo 端到端手测**

重启 demo（`MCPGW_DASH_ADMIN=demo-admin-2026 ./target/debug/mcpgw --config mcpgw.toml serve`），浏览器逐项核对：
- Config **默认进 Form**；整体在圆角 panel 内，顶部 segmented(Raw│Form) + Save/Reload。
- 左段导航 sidebar 风格（accent 左条 + 渐变 active）；子表是柔和 sub-panel；字段/数组/错误均精致、深色不突兀。
- 功能不回归：Raw↔Form 切换与同步、strategy 切换子表出现/清理、upstream 增删与 transport 切换、headers 增删、校验红框 + Save 禁用、Save 热重载结果卡片、解析失败提示。

## 完成标准

- 4 个 task 全部提交；`npm run test` 28 passed、`dist` 字节级可复现；后端 `cargo build --locked` + `cargo test` 不受影响；demo 视觉与 dashboard 统一、默认 Form、功能零回归。
