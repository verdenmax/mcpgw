# mcpgw Dashboard Config 表单视觉重设计（对齐深色设计语言）

- 日期：2026-06-23
- 状态：设计已批准，待实施
- 关联：在 `2026-06-23-mcpgw-dashboard-config-form-editor-design.md`（结构化表单模式）之上做**纯视觉**打磨

## 1. 背景与动机

Config 的结构化表单功能完整、逻辑正确（28 vitest + 契约测试），但视觉与 dashboard 整体深色设计语言**不统一、显突兀**：

- 默认进入 **raw** 模式（期望默认 **Form**）；
- 表单**"裸"平铺**，未像其他页面用 `.card`/`.table-wrap` 圆角容器包裹 → 深色背景下字段散乱；
- 左段导航 `.cfg-navitem` 是**朴素 border 按钮**，与精致的主 sidebar（active 渐变 + accent 左条）割裂；
- 段用**原生 `<fieldset>` + `<legend>`**，深色下 legend/边框割裂，最突兀；
- `Raw│Form` 切换用**朴素 `.admbtn`**，dashboard 已有更精致的 pill/chip；
- input/间距密集，缺 card 的呼吸感与 hover/focus 微交互。

dashboard 已有成熟的设计语言可复用：圆角层次 `--r-sm/md/lg`、`.card`（hover 上浮 + shadow + accent 顶条）、sidebar active 渐变 + accent 左条、`.chip`(pill, active `--brand-fill`)、`.table-wrap` 圆角包裹、`--accent-grad`、`.14s` 微交互、`tabular-nums`。

## 2. 目标 / 非目标

**目标**：Config 默认 **Form** 模式；表单全面对齐 dashboard 深色设计语言（容器化、精致导航、segmented 切换、sub-panel 段、字段统一、数组行卡片化、统一微交互），消除"突兀/不统一"。

**非目标（严格不变）**：校验规则（`validateModel`）、raw↔model 同步（`parseToml`/`stringifyToml`/`pruneModel`）、Save/PUT 数据流、后端代码、`configSchema`/枚举。这是**纯视觉 + 默认模式**改动；现有 28 个 vitest 必须保持全绿（逻辑零改动）。

## 3. 设计（8 点，均复用现有 token/语言）

| # | 项 | 设计 | 复用 |
| --- | --- | --- | --- |
| ① | **容器化** | Config 主体放进圆角 panel（`--r-lg` border + `--panel` 背景 + `--shadow-sm`）；顶部工具条：左 `Raw│Form` 切换、右 `Save`/`Reload` | `.card`/`.table-wrap` 语言 |
| ② | **左段导航** | 复用 sidebar 语言：active = accent 渐变背景 + **accent 左条**(`::before`)，hover = `--hover`；段名 + 🔥/⟳ badge；轻量、区别主 sidebar | `.sidebar li.active`（:158-163） |
| ③ | **Raw│Form 切换** | **segmented control**：pill 容器内两段，active 段 `--accent`/`--brand-fill` 填充 | `.chip.active`（:258） |
| ④ | **段/子表** | 原生 `<fieldset>`+`<legend>` → 柔和 **sub-panel**（`--r-md` + `--panel-2` 背景 + 小标题 label，无原生 legend 割裂） | `--panel-2`/`--r-md` |
| ⑤ | **字段** | input/select 统一 `--r-sm`、`--panel-2` 背景、hover border-`--border-hover`、focus `--ring`；label 小号 muted 在上、控件在下，字段间 `--s3` 呼吸；switch 行对齐；数值 `tabular-nums` | `--ring`/`.num` |
| ⑥ | **数组行**（upstream/api_key/headers） | 每行轻卡片化（`--hover`/border 圆角）；✕/＋ 按钮统一 `.iconbtn` 风格 | `.iconbtn`（:134-138） |
| ⑦ | **错误列表** | danger 柔和卡片（`--r-sm` 圆角 + `--danger-bg`/`-bd` + 图标） | `.error`（:166） |
| ⑧ | **微交互** | 统一 `.14s` transition（hover/active/focus），与 dashboard 一致 | 全局 transition 习惯 |

### 结构示意（保持布局 A'：左导航 + 右表单，整体套 panel）

```
╭─ Config ─────────────────────────────────────────╮   ← --r-lg panel + --shadow-sm
│ (  Raw  |  ▣ Form  )                [ Save ][ Reload ]│   ← segmented 切换 + 工具条
├──────────────┬────────────────────────────────────┤
│ ▎Retrieval 🔥│  strategy [ bm25 ▾ ]   top_k [  8 ] │
│  Server    ⟳ │  ╭ vector ───────────────────────╮ │   ← sub-panel(--panel-2)
│  Audit     ⟳ │  │ model [____]  api_key_env [__] │ │
│  Dashboard ⟳ │  ╰────────────────────────────────╯ │
│  Upstreams 🔥│                                     │
╰──────────────┴────────────────────────────────────╯
  accent 左条+渐变        字段呼吸/圆角/focus ring/微交互
```

## 4. 范围（实施边界）

- **`crates/dashboard/ui/src/app.css`**：重写/扩展 `.cfg-*` 规则（容器、导航、segmented、sub-panel、字段、数组行、错误、微交互）。类名仍全局唯一 `.cfg-` 前缀。
- **组件结构微调**（不改逻辑/绑定/校验）：
  - `Config.svelte`：默认 `view = "form"`（M5 默认 raw 改为 form）；工具条/容器 wrapper class；segmented 切换标记（`role="tablist"`/`aria-pressed` 已在）。
  - `FormEditor.svelte`：导航/pane 容器 class。
  - `SectionRetrieval/Server/Upstreams.svelte`：`<fieldset><legend>` → `<div class="cfg-sub"><div class="cfg-sub-h">…`（sub-panel），数组行 wrapper class、按钮改 `.iconbtn`。
  - 其余 Section（Audit/Dashboard）：字段 class 微调。
- **重建并提交 `dist/`**（字节级可复现）。

## 5. 验收 / Gates

- `npm run test` → **28 passed**（逻辑零改动，测试不变）。
- `npm run build` exit 0；`npm ci && npm run build` 后 `git status dist` 空（字节级可复现）。
- 后端 `cargo fmt/clippy/test/build` 不受影响（前端-only）。
- **视觉手测（demo）**：默认进 Form；容器/左导航/segmented/sub-panel/字段/数组行/错误/微交互均与 dashboard 其他页面观感一致、深色下不突兀；raw 模式、各段编辑、校验红框、Save 热重载、解析失败提示功能不变。

## 6. 风险与缓解

- **回归功能**：纯视觉改动，但组件结构微调（fieldset→div）可能误伤绑定。缓解：保持所有 `bind:`/`onclick`/`$effect` 不变，仅改包裹元素/class；28 vitest（纯函数）+ demo 手测兜底。
- **dist 漂移**：每次组件/CSS 改动重建提交 dist，最终 `npm ci && build` 验证可复现。
- **过度设计**：严格复用现有 token/类，不引入新色板/字体；类名全局唯一防级联冲突。
