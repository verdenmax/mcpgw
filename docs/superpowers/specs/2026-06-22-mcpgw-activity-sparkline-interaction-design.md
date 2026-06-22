# Dashboard Activity 柱状图交互（per-bar 计数 + 点击筛选）设计

> 状态：已设计待评审 · 日期 2026-06-22 · `feat/dashboard-activity` 后续增量（纯前端）

## 目标

在已落地的 Activity sparkline 上加两项交互：
1. **每根非零柱显示调用数**（标签）。
2. **点击某柱 → 把 Calls 列表筛到该柱的时间窗** `[t, t+bucket_ms)`。
   - Calls 页内点击：直接本页筛选。
   - Overview 页内点击：跳转到 Calls 并自动带上该窗（用户选 B）。

**纯前端**——后端 `/api/calls` 的 `since`/`until` 已支持（`since_ms`/`until_ms` 均为闭区间 `[since,until]`，
见 `crates/dashboard/src/calls.rs:76`），`/api/activity` 不变、无任何后端改动。

## 背景

- 现 `Sparkline.svelte` 是一段「拉伸 SVG（`preserveAspectRatio="none"`）」——文字会被横向拉伸变形，且整段
  SVG 难做逐柱点击/标签。
- `/api/activity` 已返回 `bucket_ms` 与每个 `bucket.t`（桶起始 epoch ms）；柱 i 覆盖半开窗 `[t_i, t_i+bucket_ms)`。
- Calls 当前时间范围是「滚动窗」：`since = Date.now()-rangeMs` 在 `loadCalls()` 请求时计算（已修的滑动窗）。
  本次新增「绝对桶窗」选择，与滚动窗**互斥**。

## 非目标（YAGNI）

- 不在柱上做选中高亮（Activity 每 tick 重取、桶边界滑动，t 精确匹配不稳定）——用筛选区的「selected bucket」
  chip 表达当前选择即可。
- 不改 `/api/activity`、不改后端、不加 history 聚合。
- 零计数柱不可点（也不显示标签）。

---

## 组件设计

### `Sparkline.svelte`（重写为 CSS flex 柱）

- **Props**：`buckets`（`[{t,total,errors}]`）、`bucketMs: number`、`onpick?: (since:number, until:number) => void`。
- 渲染一行 24 列（flex，每列 `flex:1`）：
  - `max = Math.max(1, ...buckets.map(b=>b.total))`。
  - 每列：
    - **零计数柱** → 一条贴底细基线 `<div class="bar0">`（非交互、无标签）。
    - **非零柱** → `<button class="barbtn" onclick={() => onpick?.(b.t, b.t + bucketMs - 1)} title="…">`，内含：
      - 顶部 `<span class="barnum">{b.total}</span>`（小字号，仅非零显示）。
      - 柱体 `<div class="bar" style="height:{total/max*100}%">`，底部叠红色错误段
        `<div class="barerr" style="height:{errors/total*100}%">`（错误段占该柱高度的 errors/total）。
    - `title` = `{when(b.t)} · {b.total} calls{b.errors? ", N err":""}`。
  - 整体外层 `<div class="sparkbars" role="img" aria-label="…">`（汇总 N calls / M errors）。
- 无 `onpick` 时（防御）柱仍渲染但 onclick 安全空跑（`onpick?.()`）。无 `{@html}`。

### `Activity.svelte`

- 新增 prop `onpick?`，透传给 `<Sparkline buckets={data.buckets} bucketMs={data.bucket_ms} {onpick} />`。
- 其余不变（仍按 `refresh.tick` 拉取、`sections` 控制块）。

### `lib/bucketSel.svelte.js`（新，跨页传递）

```js
// Overview 点柱后暂存所选绝对窗；Calls 初始化时消费一次。纯内存、无持久化。
export const pendingBucket = $state({ since: null, until: null });
```

### `Overview.svelte`

- 给 `<Activity>` 传 `onpick`：
  ```js
  function pickBucket(since, until) { pendingBucket.since = since; pendingBucket.until = until; location.hash = "#/calls"; }
  ```
  即「设暂存窗 → 跳 Calls」。

### `Calls.svelte`

- 新增 `let bucketSel = $state(null);`（`{since, until}` 或 `null`）。
- **初始化消费暂存**：脚本顶层（或 onMount）一次性：
  ```js
  if (pendingBucket.since != null) { bucketSel = { since: pendingBucket.since, until: pendingBucket.until };
    pendingBucket.since = null; pendingBucket.until = null; offset = 0; }
  ```
- **query/loadCalls 互斥逻辑**：
  - `query` 的 `$derived.by`：`if (bucketSel) { q.set("since", String(bucketSel.since)); q.set("until", String(bucketSel.until)); }`
    （绝对值、可安全进 memoized derived）。
  - `loadCalls()` 的请求时滚动 `since` 仅在**无 bucketSel** 时附加：
    `const since = (!bucketSel && rangeMs > 0) ? \`&since=${Date.now()-rangeMs}\` : "";`
  - in-flight 守卫保持 `reqQ===query && reqRange===rangeMs`（bucketSel 进了 query，故 query 变化即被守卫覆盖）。
- **互斥**：选桶覆盖滚动窗（`bucketSel` 存在时 `rangeMs` 不参与）；点任一时间范围 chip → `setRange` 里清 `bucketSel`。
- **本页点柱**：给 `<Activity>` 传 `onpick = (since, until) => { bucketSel = { since, until }; offset = 0; }`。
- **清除 chip**：当 `bucketSel` 存在，在时间范围 chips 行尾显示 `<button class="chip active">bucket: HH:MM:SS–HH:MM:SS ✕</button>`，
  点击 `bucketSel = null; offset = 0;`。
- **空状态**：`anyFilter || rangeMs > 0 || bucketSel` 视为「已筛」（沿用既有「No calls match these filters」分支）。
- **效果触发**：刷新效果加 `void bucketSel;`（`$effect(() => { void query; void rangeMs; void bucketSel; refresh.tick; loadCalls(); })`），
  确保点柱/清除即时重取（即便 query 未变，如 offset 已 0）。

### `app.css`

加 flex sparkline 样式：`.sparkbars`（flex、gap、固定高如 56px）、`.barcol`（flex:1、列布局、底对齐）、
`.barnum`（小字号、居中、`var(--muted)`）、`.barbtn`（透明按钮、cursor、hover 提亮、focus 环）、
`.bar`（蓝紫渐变、圆角顶）、`.barerr`（`var(--danger)`、贴底）、`.bar0`（1–2px 贴底基线）。

---

## 测试与验证

- 纯前端、无新后端逻辑 → 无新 Rust 单测；保持 `assets::` 3/3、构建 **0 警告**（柱是 `<button>`、零柱是 `<div>` 无 onclick，
  无 a11y 告警；无 `{@html}`）、dist 可复现。
- 手动/演示验证：① 非零柱显示数目；② Calls 内点柱→表格筛到该窗 + 出现 selected-bucket chip + 可清除；
  ③ Overview 点柱→跳 Calls 且自动筛到该窗；④ 选桶与时间范围 chip 互斥；⑤ 滚动窗仍随刷新滑动（无 bucketSel 时）。
- 四道门禁全绿（虽无 Rust 改动，仍跑确保未破坏 assets/嵌入）。

## 文档（随码同提交）

- **L3 `docs/L3-details/dashboard.md`**：在「活动聚合」段补一句：sparkline 为可交互 flex 柱（非零柱显示计数、点击
  以 `since`/`until` 闭区间筛 Calls 到该桶窗 `[t, t+bucket_ms-1]`；Overview 经 `pendingBucket` 跨页带窗）。
- **L4 `docs/L4-api/dashboard.md`**：前端组件备注 `Sparkline` 的 `onpick(since,until)` 与 `bucketMs` props、
  `bucketSel`/`pendingBucket` 的 Calls 桶筛选；后端无新增（注明复用已有 `until`）。

## 完成判据（DoD）

- 非零柱显示数目；点柱在 Calls 内/从 Overview 都能把表格精确筛到该桶闭区间窗。
- 选桶与滚动时间范围互斥、可一键清除；空状态文案正确。
- 后端零改动（复用 `since`/`until`）；构建 0 警告、assets 3/3、dist 可复现、四门禁绿；L3/L4 同步。
- subagent-driven：每 task spec+质量双审查；最后整分支 audit → 由用户决定合并。
