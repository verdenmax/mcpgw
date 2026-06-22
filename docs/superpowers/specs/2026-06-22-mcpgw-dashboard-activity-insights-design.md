# Dashboard 活动聚合洞察（Activity Insights）设计

> 状态：已设计待评审 · 日期 2026-06-22 · 子系统 A（只读 dashboard）增量

## 目标

把 dashboard 从「列举」升级到「洞察」：在现有只读面板上，基于 live 调用环新增一个**后端聚合端点**
`/api/activity`，前端据此渲染①调用量/错误**趋势 sparkline**、②（复用现有 `since`）**时间范围过滤**、
③**error_kind 列 + 分布**、④**最慢调用 / 最忙工具排行榜**。全程**只读、仅元数据**（绝不含 args/result
内容），不破坏现有隐私不变量。

## 背景与现状（已核实）

- live 调用环 `CallRingSink`（`crates/dashboard/src/calls.rs`）持有 `StoredCall { seq, record: CallRecord, content }`；
  `CallRecord` 含 `ts_unix_ms / meta_tool / target_tool / upstream / latency_ms / outcome / error_kind / arg_bytes /
  result_bytes`——聚合所需字段**全部已在环内**，无需新增捕获。
- `/api/calls` 已支持 `since`/`until`（epoch ms）过滤（`call_filter_from_query`），但前端未暴露时间范围 UI。
- `/api/calls` 列表项含 `error_kind`（`skip_serializing_if`，error/timeout 时出现），但表格未展示。
- `per_meta_tool` 有 p50/p95/max；`per_upstream` 仅 calls/errors。
- 前端为 Svelte 5 + Vite，经 rust-embed 内嵌 `dist/`；中央刷新控制器 `refresh.svelte.js` 驱动各页轮询；
  禁用 `{@html}`（`assets::no_svelte_component_uses_raw_html` 锁死）。

## 非目标（YAGNI）

- 不做持久化时间序列 / 跨重启历史（仅基于内存环、重启即清）。
- 不做 per-upstream / per-tool 延迟分位（留待后续，需扩 MetricsSink）。
- 不做 Prometheus/OTel 导出（属 M6.T2，另立项）。
- 不引入任何前端依赖（sparkline 用纯内联 SVG）。

---

## 后端设计

### 端点

`GET /api/activity?window=<ms>`（dashboard 路由，端点数 11 → 12）。

- `window`：聚合时间窗（毫秒）。缺省 `900_000`（15 min）。clamp 到 `[60_000, 86_400_000]`（1 min–24 h）。
  解析失败 → 用缺省。窗内 = `record.ts_unix_ms >= now - window`（`now = CallRecord::now_unix_ms()`）。
- 在 `CallRingSink` 上**单次扫描**（环有界 ≤ `call_buffer`，默认 2000），用环内已有 `CallRecord` 元数据聚合。
- O(环大小) 每请求，无分配热点。

### 响应类型（`crates/dashboard/src/activity.rs`）

```rust
#[derive(Serialize)]
pub struct ActivityResponse {
    pub window_ms: u64,
    pub bucket_ms: u64,                 // window_ms / BUCKETS（固定 24）
    pub buckets: Vec<ActivityBucket>,   // 恰好 BUCKETS 个，时间升序
    pub total: u64,
    pub errors: u64,
    pub by_error_kind: Vec<KindCount>,  // count 降序
    pub slowest: Vec<SlowCall>,         // 最多 TOP_N=5，latency 降序
    pub busiest_tools: Vec<ToolCount>,  // 最多 TOP_N=5，count 降序
}
#[derive(Serialize)]
pub struct ActivityBucket { pub t: u64, pub total: u64, pub errors: u64 } // t = 桶起始 epoch ms
#[derive(Serialize)]
pub struct KindCount { pub kind: String, pub count: u64 }
#[derive(Serialize)]
pub struct SlowCall { pub id: String, pub label: String, pub meta_tool: String,
                      pub latency_ms: u64, pub outcome: String }
#[derive(Serialize)]
pub struct ToolCount { pub name: String, pub count: u64 }
```

`ActivityResponse` **不含任何 args/result 内容字段**——隐私不变量由「类型里没有内容字段」结构性保证，
另加单测断言其序列化 JSON 不含 `"args"`/`"result"`。

### 聚合算法（纯函数，便于单测）

`activity::aggregate(records: &[AggInput], window_ms: u64, now: u64) -> ActivityResponse`，其中
`AggInput` 是从 `StoredCall` 投影出的轻量元组/结构（`seq→id`、`ts_unix_ms`、`meta_tool`、`target_tool`、
`latency_ms`、`outcome`、`error_kind`），由 `CallRingSink::activity` 在锁内构造后传入。

常量：`const BUCKETS: usize = 24; const TOP_N: usize = 5;`

1. `bucket_ms = (window_ms / BUCKETS).max(1)`；窗起点 `start = now - bucket_ms * BUCKETS`（保证 24 桶整覆盖，
   末桶含 `now`）。`buckets[i].t = start + i*bucket_ms`，total/errors 初始 0。
2. 遍历 `records`，仅取 `ts >= start`：
   - `idx = ((ts - start) / bucket_ms)`，`idx.min(BUCKETS-1)`（钳制末桶）；`buckets[idx].total += 1`；
     `outcome != "ok"` 时 `buckets[idx].errors += 1`。
   - `total += 1`；非 ok → `errors += 1`。
   - `error_kind` 为 `Some(k)` 时 `kind_map[k] += 1`。
   - `target_tool` 为 `Some(t)` 时 `tool_map[t] += 1`（`search_tools` 无 target，不计入 busiest）。
   - 收集 `(latency_ms, id, label, meta_tool, outcome)` 入 slowest 候选；`label = target_tool.unwrap_or(meta_tool)`。
3. `by_error_kind` = `kind_map` 按 count 降序（并列按 kind 名升序，结果稳定）。
4. `slowest` = 候选按 `latency_ms` 降序取前 `TOP_N`（并列按 ts 降序即更晚的在前，保证稳定）。
5. `busiest_tools` = `tool_map` 按 count 降序（并列按名升序）取前 `TOP_N`。
6. 空环 / 全部窗外 → total=errors=0、各 Vec 为空、buckets 仍是 24 个全 0 桶。

### 接线

- `crates/dashboard/src/calls.rs`：`pub fn activity(&self, window_ms: u64, now: u64) -> ActivityResponse`
  ——`self.ring.lock()`（自愈 poison）→ 投影窗内 `AggInput` → `activity::aggregate`。
- `crates/dashboard/src/api.rs`：`pub async fn activity(State, Query<HashMap>) -> Json<ActivityResponse>`
  ——读 `window`（解析+clamp，缺省 15min），调 `state.calls`（dashboard 未启用 calls 时返回空 `ActivityResponse`，
  与 `/api/calls` 在无环时的处理一致）。
- `crates/dashboard/src/lib.rs`：`.route("/api/activity", get(api::activity))`。

---

## 前端设计

### 新组件

- **`Sparkline.svelte`**（纯内联 SVG，无依赖）：props `buckets`（`[{t,total,errors}]`）。画 24 根**堆叠柱**：
  柱高 ∝ `total / max(total)`，其中 `errors` 段红色叠在底部、非错误段蓝紫渐变。每根 `<title>` 显示
  桶时间 + `total`/`errors`。整体 `aria-label` 汇总（如「最近 15 分钟 120 次调用、7 次错误」）。全 0 → 平基线。
- **`Activity.svelte`**：按 `refresh.tick` 拉 `/api/activity?window=<window>`，渲染。props：
  - `window: number`（毫秒）。
  - `sections: string`（逗号分隔，含 `spark` / `breakdown` / `leaders`，按需渲染对应块）。
  - 取请求时序守卫（`reqW === window`，复用既有模式）；失败静默（次要数据，不抢主错误 UI）。
  - 空 → `.empty`「no activity yet」。

### 放置与改动

- **`Overview.svelte`**：指标卡之后插入 `<Activity window={900000} sections="spark,leaders" />`
  （趋势条 + 最慢/最忙两榜并排，复用 `.cards`/`.two` 风格）。
- **`Calls.svelte`**：
  - `<script>` 加 `let rangeMs = $state(900000);`（默认 15 min）。
  - 顶部时间范围 chips `[5m,15m,1h,24h,all]`：点击设 `rangeMs`（`all` → `0`）、`offset = 0`。
  - `query` 的 `$derived.by` 内：`if (rangeMs > 0) q.set("since", String(Date.now() - rangeMs));`
    （复用现有 `/api/calls` 的 `since`）。
  - 时间范围 chips 之后插入 `<Activity window={rangeMs > 0 ? rangeMs : 3600000} sections="spark,breakdown" />`
    （`all` 时 sparkline 回退 1 h 窗，避免无界）。
  - 表格 `<thead>` 与每行加 **`error_kind`** 列：`<td>{c.error_kind ?? "—"}</td>`，非空用 `--danger` 弱标
    （`class:bad={c.error_kind}`）。
- **`app.css`**：加 `.spark`/`.spark .bar`/`.lead`（排行榜行）/`.kindbar`（error_kind 分布条）等 token 化样式。
- 重建 `dist/`（`npm run build`，0 警告）。

### 交互

- 时间范围、`refresh.tick` 共同驱动：表格走 `/api/calls?...&since`，`Activity` 走 `/api/activity?window`，二者
  一致。中央 `refresh`（暂停/手动刷新）对 `Activity` 同样生效。
- 全部纯只读、无 `{@html}`；chips 为 `<button>`；sparkline 提供 `aria-label`；`prefers-reduced-motion` 既有规则覆盖。

---

## 测试

- **后端单测**（`activity.rs` 聚合纯函数 + `calls.rs` 集成）：
  - 分桶：跨多桶的时间戳落到正确桶；`now` 落末桶；窗外（`ts < start`）不计入。
  - `buckets.len() == 24` 恒成立；`bucket_ms == window/24`。
  - Top-N：>5 候选时截断且按 latency 降序；busiest 仅统计 `target_tool`（`search_tools` 不计）；
    error_kind 仅统计非 ok。
  - 并列稳定性（kind/tool 名升序、slowest 按 ts 降序）。
  - 空环 / 全窗外：total=errors=0、Vec 空、24 桶全 0。
  - **隐私**：`serde_json::to_string(&resp)` 不含 `"args"`/`"result"`。
  - `window` clamp：0 → 60_000；超大 → 86_400_000。
- **e2e**（`crates/mcpgw/tests/dashboard.rs`，mock 上游，`--ignored` + `MCPGW_REQUIRE_MOCK=1`）：
  驱动若干 `mock__echo` 调用后 `GET /api/activity`，断言 `total >= N`、`buckets` 长度 24、
  `busiest_tools` 含 `mock__echo`。
- **前端**：`assets::` 3/3 绿；构建 0 警告；`dist/` 可复现。
- **四道门禁**：`cargo fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、
  `cargo test --all-features`、`cargo build --locked` 全绿；记录 `N passed / M ignored`。

## 文档（随码同提交）

- **L1**：端点 11→12；「活动聚合洞察」一句（趋势/时间范围/error_kind/排行榜，仅元数据）。
- **L2 `dashboard.md`**：`ActivityResponse` 形状、`CallRingSink::activity`、`/api/activity` 查询串。
- **L3 `dashboard.md`**：聚合算法（固定 24 桶、窗口与 clamp、Top-N、busiest 语义、隐私边界——只元数据）。
- **L4 `dashboard.md`**：`/api/activity` 端点、`activity::aggregate`、各响应类型逐项。

## 里程碑拆分

单一计划即可（一个内聚增量）。建议 task 序：
1. 后端聚合（`activity.rs` 类型 + `aggregate` + 单测）。
2. 后端接线（`CallRingSink::activity` + `api::activity` + 路由 + 集成测 + L3/L4 文档）。
3. 前端 `Sparkline` + `Activity` 组件（+ 样式）。
4. Overview/Calls 接入（时间范围 chips + error_kind 列 + 放置）+ 重建 dist。
5. e2e + L1/L2 文档同步 + 四道门禁 + 计数回填。

执行：subagent-driven，每 task 跑 spec+质量双审查；最后整分支 audit → `--no-ff` 合并。

## 验收判据（DoD）

- `GET /api/activity` 返回 24 桶、Top5 榜、error_kind 分布、仅元数据；`window` 缺省 15min、可 clamp。
- Overview 显示趋势 + 两榜；Calls 显示时间范围过滤（默认 15m，驱动表格与 sparkline）+ error_kind 列 + 分布。
- 隐私不变量保持（响应/榜单绝不含 args/result）。
- 四道门禁 + assets + e2e 全绿；L1–L4 文档同步；dist 可复现、构建 0 警告。
