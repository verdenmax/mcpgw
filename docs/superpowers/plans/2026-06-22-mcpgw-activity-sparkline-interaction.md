# Activity Sparkline 交互（per-bar 计数 + 点击筛选）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 把 Activity sparkline 改为可交互 flex 柱：非零柱显示计数、点击把 Calls 列表筛到该柱时间窗（Calls 内本页筛、Overview 跳转带窗）。

**Architecture:** 纯前端。`Sparkline.svelte` 重写为 CSS flex 柱（非零柱=`<button>` 带计数 + onpick；零柱=细基线）；`Activity.svelte` 透传 `onpick`/`bucketMs`；新 `bucketSel.svelte.js` 跨页暂存窗；Overview 点柱设暂存窗+跳 `#/calls`，Calls 点柱设本页 `bucketSel`（绝对 `since`/`until` 闭区间，与滚动时间范围互斥）。后端零改动（复用已有 `since`/`until`）。

**Tech Stack:** Svelte 5 runes + Vite（rust-embed 内嵌 dist）。

参考 spec：`docs/superpowers/specs/2026-06-22-mcpgw-activity-sparkline-interaction-design.md`

---

## 文件结构

- **Rewrite** `crates/dashboard/ui/src/lib/Sparkline.svelte` —— flex 柱 + 计数标签 + `onpick`/`bucketMs` props。
- **Modify** `crates/dashboard/ui/src/lib/Activity.svelte` —— 新增 `onpick` prop 并透传给 Sparkline。
- **Create** `crates/dashboard/ui/src/lib/bucketSel.svelte.js` —— `pendingBucket` 跨页暂存窗。
- **Modify** `crates/dashboard/ui/src/lib/Overview.svelte` —— 点柱设暂存窗 + 跳 `#/calls`。
- **Modify** `crates/dashboard/ui/src/lib/Calls.svelte` —— `bucketSel` 桶筛（query/loadCalls 互斥、清除 chip、effect、空状态）。
- **Modify** `crates/dashboard/ui/src/app.css` —— flex sparkline 样式。
- **Modify** `docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`。

---

## Task 1：Sparkline 重写为 flex 柱（计数 + onpick）+ Activity 透传 + 样式

**Files:**
- Rewrite: `crates/dashboard/ui/src/lib/Sparkline.svelte`
- Modify: `crates/dashboard/ui/src/lib/Activity.svelte`、`crates/dashboard/ui/src/app.css`

> 本 task 后,Overview/Calls 暂未传 `onpick`，柱可点但 onclick 安全空跑（`onpick?.()`）——预期。

- [ ] **Step 1: 重写 `Sparkline.svelte`（整文件替换为下列内容）**

```svelte
<script>
  // 24 根可交互 flex 柱：非零柱顶部显示计数、可点击 onpick(since, until)；零柱细基线（非交互）。
  // 柱体蓝紫渐变、底部红色错误段（占该柱高度的 errors/total）。纯 DOM，无依赖、无 {@html}。
  import { when } from "./format.js";
  let { buckets = [], bucketMs = 0, onpick } = $props();
  const max = $derived(Math.max(1, ...buckets.map((b) => b.total)));
  const totalCalls = $derived(buckets.reduce((a, b) => a + b.total, 0));
  const totalErr = $derived(buckets.reduce((a, b) => a + b.errors, 0));
  function title(b) {
    return `${when(b.t)} · ${b.total} calls${b.errors ? `, ${b.errors} err` : ""}`;
  }
</script>

<div class="sparkbars" role="img" aria-label={`${totalCalls} calls, ${totalErr} errors over the window`}>
  {#each buckets as b}
    {#if b.total > 0}
      <button class="barcol barbtn" title={title(b)} onclick={() => onpick?.(b.t, b.t + bucketMs - 1)}>
        <span class="barnum">{b.total}</span>
        <span class="bar" style="height:{(b.total / max) * 100}%">
          {#if b.errors}<span class="barerr" style="height:{(b.errors / b.total) * 100}%"></span>{/if}
        </span>
      </button>
    {:else}
      <span class="barcol"><span class="bar0"></span></span>
    {/if}
  {/each}
</div>
```

- [ ] **Step 2: `Activity.svelte` 加 `onpick` prop 并透传**

2a. 把
```svelte
  let { window: win, sections = "spark" } = $props();
```
改为
```svelte
  let { window: win, sections = "spark", onpick } = $props();
```

2b. 把
```svelte
        <Sparkline buckets={data.buckets} />
```
改为
```svelte
        <Sparkline buckets={data.buckets} bucketMs={data.bucket_ms} {onpick} />
```

- [ ] **Step 3: `app.css` 追加 flex sparkline 样式（在 `.spark-legend` 规则之后；旧 `.spark`/SVG 规则可保留无害，但本 task 用新类）**

```css
.sparkbars { display: flex; align-items: flex-end; gap: 2px; height: 56px; }
.barcol { flex: 1; min-width: 0; height: 100%; display: flex; flex-direction: column;
          justify-content: flex-end; align-items: stretch; }
.barbtn { background: none; border: 0; padding: 0; cursor: pointer; }
.barnum { font-size: 9px; line-height: 1; text-align: center; color: var(--muted);
          font-variant-numeric: tabular-nums; margin-bottom: 2px; }
.bar { width: 100%; max-height: calc(100% - 13px); border-radius: 2px 2px 0 0;
       background: linear-gradient(180deg, var(--accent), var(--accent-2));
       display: flex; align-items: flex-end; }
.barbtn:hover .bar { filter: brightness(1.18); }
.barbtn:focus-visible { outline: none; box-shadow: var(--ring); border-radius: var(--r-sm); }
.barerr { width: 100%; background: var(--danger); border-radius: 2px 2px 0 0; }
.bar0 { height: 2px; width: 100%; background: var(--border); border-radius: 1px; }
```

- [ ] **Step 4: 构建 + assets 测试**

Run: `cd crates/dashboard/ui && npm run build 2>&1 | tail -6 && cd ../../.. && cargo test -p dashboard assets:: 2>&1 | grep -E '^test result:'`
Expected: 构建成功、**0 警告**（非零柱是 `<button>`、零柱 `<span>` 无 onclick → 无 a11y 告警）；`assets::` 3 passed。

- [ ] **Step 5: Commit**

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): interactive flex Sparkline (per-bar count + onpick)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：bucketSel 跨页态 + Overview 点柱跳转 + Calls 桶筛选

**Files:**
- Create: `crates/dashboard/ui/src/lib/bucketSel.svelte.js`
- Modify: `crates/dashboard/ui/src/lib/Overview.svelte`、`crates/dashboard/ui/src/lib/Calls.svelte`

- [ ] **Step 1: 创建 `bucketSel.svelte.js`**

```js
// Overview 点 sparkline 柱后暂存所选绝对窗（since/until, epoch ms）；Calls 初始化时消费一次。
// 纯内存、无持久化。Svelte 5 universal reactivity（.svelte.js 里的 $state）。
export const pendingBucket = $state({ since: null, until: null });
```

- [ ] **Step 2: `Overview.svelte` —— 点柱设暂存窗 + 跳 Calls**

2a. 引入（在 `import Activity from "./Activity.svelte";` 之后）：
```svelte
  import { pendingBucket } from "./bucketSel.svelte.js";
```

2b. 在 `<script>` 内（`load` 定义附近、`onMount`/`$effect` 之前的合适处）加函数：
```svelte
  function pickBucket(since, until) {
    pendingBucket.since = since;
    pendingBucket.until = until;
    location.hash = "#/calls";
  }
```

2c. 把
```svelte
  <Activity window={900000} sections="spark,leaders" />
```
改为
```svelte
  <Activity window={900000} sections="spark,leaders" onpick={pickBucket} />
```

- [ ] **Step 3: `Calls.svelte` 脚本 —— bucketSel 态 + 消费暂存 + query/loadCalls 互斥 + setRange/effect**

3a. 引入（在 `import Activity from "./Activity.svelte";` 之后）：
```svelte
  import { pendingBucket } from "./bucketSel.svelte.js";
```

3b. 加 `bucketSel` 态并**一次性消费暂存窗**（把
```svelte
  let rangeMs = $state(900000); // 时间范围(ms)；0 = all。默认 15min
```
改为）
```svelte
  let rangeMs = $state(900000); // 时间范围(ms)；0 = all。默认 15min
  let bucketSel = $state(null); // {since, until} 绝对窗（点柱所选）；与 rangeMs 互斥
  // 从 Overview 点柱跳转过来时，组件初始化消费一次暂存窗。
  if (pendingBucket.since != null) {
    bucketSel = { since: pendingBucket.since, until: pendingBucket.until };
    pendingBucket.since = null;
    pendingBucket.until = null;
  }
```

3c. `query` 里**无条件读 `bucketSel`**（避免 null→设的条件依赖陷阱）。把
```svelte
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
    q.set("limit", String(LIMIT));
```
改为
```svelte
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
    const bs = bucketSel; // 无条件读 -> query 始终把 bucketSel 当依赖
    if (bs) { q.set("since", String(bs.since)); q.set("until", String(bs.until)); }
    q.set("limit", String(LIMIT));
```

3d. `loadCalls` 的滚动 `since` 仅在**无 bucketSel** 时附加。把
```svelte
    const since = rangeMs > 0 ? `&since=${Date.now() - rangeMs}` : "";
```
改为
```svelte
    const since = !bucketSel && rangeMs > 0 ? `&since=${Date.now() - rangeMs}` : "";
```

3e. `setRange` 清 bucketSel（时间范围覆盖桶选）。把
```svelte
  function setRange(ms) { rangeMs = ms; offset = 0; }
```
改为
```svelte
  function setRange(ms) { rangeMs = ms; bucketSel = null; offset = 0; }
```

3f. 刷新 effect 显式跟踪 bucketSel（belt-and-suspenders）。把
```svelte
  $effect(() => { void query; void rangeMs; refresh.tick; loadCalls(); });
```
改为
```svelte
  $effect(() => { void query; void rangeMs; void bucketSel; refresh.tick; loadCalls(); });
```

- [ ] **Step 4: `Calls.svelte` 标记 —— 范围 chip 互斥高亮 + 清除 chip + Activity onpick**

4a. 时间范围 chip：选桶时不高亮范围 chip。把
```svelte
  {#each [["5m", 300000], ["15m", 900000], ["1h", 3600000], ["24h", 86400000], ["all", 0]] as [lbl, ms]}
    <button class="chip" class:active={rangeMs === ms} onclick={() => setRange(ms)}>{lbl}</button>
  {/each}
</div>
<Activity window={rangeMs > 0 ? rangeMs : 3600000} sections="spark,breakdown" />
```
改为
```svelte
  {#each [["5m", 300000], ["15m", 900000], ["1h", 3600000], ["24h", 86400000], ["all", 0]] as [lbl, ms]}
    <button class="chip" class:active={!bucketSel && rangeMs === ms} onclick={() => setRange(ms)}>{lbl}</button>
  {/each}
  {#if bucketSel}<button class="chip active" onclick={() => { bucketSel = null; offset = 0; }}>bucket: {new Date(bucketSel.since).toLocaleTimeString()}–{new Date(bucketSel.until).toLocaleTimeString()} ✕</button>{/if}
</div>
<Activity window={rangeMs > 0 ? rangeMs : 3600000} sections="spark,breakdown"
          onpick={(since, until) => { bucketSel = { since, until }; offset = 0; }} />
```

4b. 空状态把 `bucketSel` 也视为「已筛」。把
```svelte
      {#if anyFilter || rangeMs > 0}<div>No calls match these filters</div><div class="hint">adjust or clear the filters above</div>
```
改为
```svelte
      {#if anyFilter || rangeMs > 0 || bucketSel}<div>No calls match these filters</div><div class="hint">adjust or clear the filters above</div>
```

- [ ] **Step 5: 构建 + assets 测试**

Run: `cd crates/dashboard/ui && npm run build 2>&1 | tail -6 && cd ../../.. && cargo test -p dashboard assets:: 2>&1 | grep -E '^test result:'`
Expected: 构建成功、**0 警告**；`assets::` 3 passed。

- [ ] **Step 6: Commit**

```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): click sparkline bar to filter Calls to that bucket window

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：L3/L4 文档同步 + 四道门禁

**Files:**
- Modify: `docs/L3-details/dashboard.md`、`docs/L4-api/dashboard.md`

> 纯前端改动、无 Rust 逻辑变化 → 无新 Rust 单测、测试计数不变（仍 283/5）。

- [ ] **Step 1: L3 文档（`docs/L3-details/dashboard.md`，READ 后改）**

在「活动聚合」段补一句（忠实于代码）：sparkline 现为**可交互 flex 柱**——非零柱顶部显示该桶调用数、点击以
`since`/`until`**闭区间** `[t, t+bucket_ms-1]` 把 Calls 列表筛到该桶窗；Calls 内点柱直接本页筛、Overview 点柱经
`pendingBucket` 跨页带窗跳 `#/calls`。桶筛（绝对 since+until）与滚动时间范围**互斥**（选范围 chip 清桶选、反之亦然），
有「selected bucket」chip 可一键清除。后端零改动（复用既有 `since`/`until`）。

- [ ] **Step 2: L4 文档（`docs/L4-api/dashboard.md`，READ 后改）**

在前端组件/dashboard 段补：`Sparkline` 组件 props `buckets`/`bucketMs`/`onpick(since, until)`（非零柱可点，回调绝对窗）；
`Activity` 透传 `onpick`；`bucketSel.svelte.js` 的 `pendingBucket` 跨页暂存窗；Calls 的 `bucketSel` 桶筛（query 无条件读
`bucketSel`、loadCalls 滚动 since 仅在无 bucketSel 时附加）。注明**后端无新增**（`/api/calls` 的 `since`/`until` 既有、`/api/activity` 不变）。

- [ ] **Step 3: 四道门禁 + dist 可复现**

```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿（预期 **283 passed / 5 ignored** 不变）。并
`cd crates/dashboard/ui && npm run build && cd ../../.. && git status --short crates/dashboard/ui/dist`（应为空）。

- [ ] **Step 4: Commit**

```bash
git add docs/L3-details/dashboard.md docs/L4-api/dashboard.md
git commit -m "docs: sync L3/L4 for interactive sparkline + Calls bucket filter

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 完成判据（DoD）

- 非零柱显示调用数；Calls 内点柱→表格精确筛到该桶闭区间窗 + 出现可清除的 selected-bucket chip。
- Overview 点柱→跳 `#/calls` 且自动筛到该窗。
- 桶筛与滚动时间范围互斥；无 bucketSel 时滚动窗仍随刷新滑动；空状态文案正确。
- 后端零改动；构建 **0 警告**、`assets::` 3/3、dist 可复现、四门禁绿（283/5）；L3/L4 同步。
- subagent-driven：每 task spec+质量双审查；最后整分支 audit（覆盖本增量 + 既有 activity 特性）→ 由用户决定收尾。
