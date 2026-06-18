# M2：调用内容过滤（payload filtering）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `/api/calls` 加**内容过滤**——`q`（自由文本子串，扫 args+result）+ `arg_key`/`arg_val`（结构化，递归找 args 里键=值），**只对 `source=live`**（history 无内容、自动忽略）；并在主 Calls 页与详情页 Recent-calls 列表加过滤 UI。

**Architecture:** `CallFilter` 增 `q`/`arg_key`/`arg_val`；`matches()` 仅当 `CallItem` 携带内容（`args` 为 `Some`，即 live 且本次需要内容）时才应用内容过滤——history/列表轻量项 `args=None`，内容过滤天然跳过。`CallRingSink::query` 仅在**存在内容过滤时**才 `to_item(true)` 构建内容用于过滤、随后为列表剥离内容，保证无内容过滤的常规轮询仍走轻量 `to_item(false)` 路径。

**Tech Stack:** Rust（dashboard）、Svelte 5 + Vite（过滤 UI + dist 入库）。

**关键约束：**
- **内容过滤只对 live**：history（audit 回放）项无内容（`args=None`），内容过滤被忽略——无需在 api 层特判，由 `matches` 的 `if let Some(args)` 门控自然实现。
- **轻量常规路径**：无内容过滤时 `query` 仍 `to_item(false)`（不 clone 内容），与 M1 性能一致；仅当 `q`/`arg` 存在时才付内容构建成本。
- **列表仍不含内容**：有内容过滤时为过滤而构建内容，但分页后**剥离**（`args/result=None`），`/api/calls` 列表始终不返回内容。
- `arg_key/arg_val` 解析 args JSON 递归找键；截断/非法 JSON → 不命中（best-effort，已文档化）。
- **XSS/dist**：前端只 `{表达式}`、无 `{@html}`；改 ui/src 必 `npm run build` 并提交 dist。

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/dashboard/src/calls.rs` | `CallFilter` 加 `q/arg_key/arg_val`；`matches` 内容过滤；`query` 的 `want_content`；`content_contains`/`args_key_value_matches` helper | 修改 |
| `crates/dashboard/src/api.rs` | `call_filter_from_query` 读 `q/arg_key/arg_val` | 修改 |
| `crates/dashboard/ui/src/lib/Calls.svelte` | 内容搜索框 + arg key/value 输入（history 禁用） | 修改 |
| `crates/dashboard/ui/src/lib/UpstreamDetail.svelte` / `ToolDetail.svelte` | Recent-calls 加 outcome chips + 内容搜索框 | 修改 |
| `crates/dashboard/ui/dist/**` | 构建产物 | 重新生成 |
| `crates/mcpgw/tests/dashboard.rs` | e2e：`?q=` / `?arg_key=&arg_val=` 命中 | 修改 |
| `docs/L4-api/dashboard.md` / `docs/L3-details/dashboard.md` / `docs/L1-overview.md` | 分层文档同步 | 修改 |

---

## Task 1：后端 —— `CallFilter` 内容过滤 + `query` want_content

**Files:**
- Modify: `crates/dashboard/src/calls.rs`（`CallFilter`、`matches`、`query`、两 helper）
- Modify: `crates/dashboard/src/api.rs`（`call_filter_from_query`）
- Test: `crates/dashboard/src/calls.rs`

- [ ] **Step 1: 写失败测试**

在 `crates/dashboard/src/calls.rs` 测试模块追加（复用 M1 的 `rec`/`content` helper；新增带不同内容的 helper）：
```rust
    fn content_of(args: &str, result: &str) -> observe::CallContent {
        observe::CallContent { args: args.into(), args_truncated: false, result: result.into(), result_truncated: false }
    }

    #[test]
    fn query_free_text_filters_over_args_and_result() {
        let ring = CallRingSink::new(10);
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__a"), CallOutcome::Ok, 1),
                    &content_of("{\"text\":\"hello\"}", "{\"ok\":1}"));
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__b"), CallOutcome::Ok, 2),
                    &content_of("{\"text\":\"world\"}", "{\"ok\":2}"));
        let f = CallFilter { q: Some("hello".into()), ..Default::default() };
        let (items, total) = ring.query(&f, 10, 0);
        assert_eq!(total, 1, "free-text matches args content");
        // list still omits content even when filtering by it:
        assert!(items[0].args.is_none(), "list omits args after content filter");
    }

    #[test]
    fn query_arg_key_value_recurses_nested_args() {
        let ring = CallRingSink::new(10);
        // call_tool nests real args under "arguments":
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__a"), CallOutcome::Ok, 1),
                    &content_of("{\"name\":\"gh__a\",\"arguments\":{\"text\":\"hi\"}}", "{}"));
        ring.record(&rec(MetaTool::CallTool, Some("gh"), Some("gh__b"), CallOutcome::Ok, 2),
                    &content_of("{\"name\":\"gh__b\",\"arguments\":{\"text\":\"bye\"}}", "{}"));
        let f = CallFilter { arg_key: Some("text".into()), arg_val: Some("hi".into()), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 1, "arg_key=text arg_val=hi matches nested");
    }

    #[test]
    fn content_filters_skip_items_without_content() {
        // Simulates a history/list-light item: no content filter built -> content filter must not exclude.
        let ring = CallRingSink::new(10);
        ring.record(&rec(MetaTool::CallTool, Some("gh"), None, CallOutcome::Ok, 1), &content_of("{}", "{}"));
        // a metadata-only filter still returns it:
        let f = CallFilter { meta_tool: Some("call_tool".into()), ..Default::default() };
        assert_eq!(ring.query(&f, 10, 0).1, 1);
    }
```

- [ ] **Step 2: 跑确认失败** — `cargo test -p dashboard calls::query_free_text` → `CallFilter` 无 `q` 字段。

- [ ] **Step 3: 实现（calls.rs）**

1. `CallFilter` 结构体加三字段（`until_ms` 之后）：
```rust
    pub q: Option<String>,
    pub arg_key: Option<String>,
    pub arg_val: Option<String>,
```
2. `matches()` 末尾、`true` 之前，加内容过滤（仅当项携带内容时应用）：
```rust
        // Content filters apply ONLY to items carrying content (live ring built with content);
        // history / list-light items have `args == None` and are NOT excluded by content filters.
        if let Some(args) = &c.args {
            if let Some(q) = &self.q {
                if !content_contains(args, c.result.as_deref(), q) {
                    return false;
                }
            }
            if let (Some(k), Some(v)) = (&self.arg_key, &self.arg_val) {
                if !args_key_value_matches(args, k, v) {
                    return false;
                }
            }
        }
```
3. 加 module 级 helper（`is_false` 附近）：
```rust
/// Case-insensitive substring of `needle` over `args` + (optional) `result`.
fn content_contains(args: &str, result: Option<&str>, needle: &str) -> bool {
    let n = needle.to_lowercase();
    args.to_lowercase().contains(&n)
        || result.map(|r| r.to_lowercase().contains(&n)).unwrap_or(false)
}

/// Parse `args` JSON and recursively check for a key `k` whose stringified value contains `v`
/// (case-insensitive). Truncated/invalid JSON -> no match (best-effort).
fn args_key_value_matches(args: &str, k: &str, v: &str) -> bool {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(args) else {
        return false;
    };
    let needle = v.to_lowercase();
    fn walk(val: &serde_json::Value, k: &str, needle: &str) -> bool {
        match val {
            serde_json::Value::Object(m) => m.iter().any(|(key, child)| {
                let hit_here = key == k && {
                    let s = match child {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    s.to_lowercase().contains(needle)
                };
                hit_here || walk(child, k, needle)
            }),
            serde_json::Value::Array(a) => a.iter().any(|x| walk(x, k, needle)),
            _ => false,
        }
    }
    walk(&val, k, &needle)
}
```
4. `query()` 改为按需构建内容：
```rust
    pub fn query(
        &self,
        filter: &CallFilter,
        limit: usize,
        offset: usize,
    ) -> (Vec<CallItem>, usize) {
        // Build content only when a content filter is active (q / arg pair); otherwise keep the
        // light, allocation-free path for ordinary list polling.
        let want_content = filter.q.is_some() || (filter.arg_key.is_some() && filter.arg_val.is_some());
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let matched: Vec<CallItem> = ring
            .iter()
            .rev()
            .map(|s| s.to_item(want_content))
            .filter(|c| filter.matches(c))
            .collect();
        drop(ring);
        let total = matched.len();
        let mut page: Vec<CallItem> = matched.into_iter().skip(offset).take(limit).collect();
        if want_content {
            // The list never returns content; we built it only to filter, now strip it.
            for c in &mut page {
                c.args = None;
                c.args_truncated = false;
                c.result = None;
                c.result_truncated = false;
            }
        }
        (page, total)
    }
```

- [ ] **Step 4: 跑确认通过** — `cargo test -p dashboard calls::`（新测试 + 既有全过）。

- [ ] **Step 5: api `call_filter_from_query` 读新参数**

`crates/dashboard/src/api.rs` 的 `call_filter_from_query` 返回的 `CallFilter { ... }` 末尾加：
```rust
        q: q.get("q").cloned(),
        arg_key: q.get("arg_key").cloned(),
        arg_val: q.get("arg_val").cloned(),
```

- [ ] **Step 6: 全后端验证 + 提交**

Run: `cargo test -p dashboard && cargo clippy -p dashboard --all-targets -- -D warnings && cargo fmt -p dashboard --check`
Expected: 全过、无 warning、无 diff。
```bash
git add crates/dashboard/src/calls.rs crates/dashboard/src/api.rs
git commit -m "feat(dashboard): /api/calls content filters (q + arg_key/arg_val, live-only)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2：前端 —— Calls 页内容过滤 UI

**Files:**
- Modify: `crates/dashboard/ui/src/lib/Calls.svelte`、`crates/dashboard/ui/src/app.css`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

在主 Calls 页加：①内容搜索框 `q` ②结构化 arg 过滤（key + value）。改任一即重置分页；`source=history` 时这些输入禁用（内容过滤只对 live）。

- [ ] **Step 1: 实现（Calls.svelte）**

1. `<script>` 状态区（`let outcome = $state("");` 之后）加：
```js
  let qtext = $state("");   // free-text content search
  let argKey = $state("");  // structured arg filter key
  let argVal = $state("");  // structured arg filter value
```
2. `query` 的 `$derived.by` 里、`q.set("limit", ...)` 之前加：
```js
    if (qtext) q.set("q", qtext);
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
```
3. 在 source/outcome chips 的 `</div>`（meta 芯片那一行之后）与 `{#if error}` 之间，插入过滤行：
```svelte
<div class="chips">
  <input class="search" placeholder="search content (args/result)…" bind:value={qtext}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  <input class="search narrow" placeholder="arg key" bind:value={argKey}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  <input class="search narrow" placeholder="value" bind:value={argVal}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  {#if source === "history"}<span class="muted">content filters apply to live only</span>{/if}
</div>
```
> `bind:value` 让输入变化驱动 `query`（`$derived`）→ `$effect` 重新拉取；`oninput` 把 `offset` 归零。`disabled` 在 history 下灰显。

- [ ] **Step 2: CSS（app.css 追加）**

```css
.search.narrow { width:120px; }
.search:disabled { opacity:0.5; cursor:not-allowed; }
```

- [ ] **Step 3: 构建 + 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd ../../.. && cargo test -p dashboard assets::`
Expected: 构建成功（a11y 警告非致命）；`assets::` 3/3。
```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): content + arg filters on the Calls page (live-only)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3：前端 —— 详情页 Recent-calls 过滤（UpstreamDetail / ToolDetail）

**Files:**
- Modify: `crates/dashboard/ui/src/lib/UpstreamDetail.svelte`、`crates/dashboard/ui/src/lib/ToolDetail.svelte`
- Regenerate+commit: `crates/dashboard/ui/dist/**`

两个详情页的 Recent-calls 当前无过滤。各加 **outcome chips + 内容搜索框**（已按 upstream/tool 固定、`source=live`）。两个组件改法一致。

- [ ] **Step 1: 实现（两个组件各自）**

以 `UpstreamDetail.svelte` 为例（`ToolDetail.svelte` 同理，把 `upstream=` 换成 `tool=`、把 `name` 语义对应）：

`<script>` 里加过滤状态与重载逻辑——把「最近调用」的拉取从 `load()` 内联改为受过滤驱动。具体：在现有 `let calls = $state([]);` 之后加：
```js
  let cOutcome = $state("");  // recent-calls outcome filter
  let cq = $state("");        // recent-calls content search
  async function loadCalls() {
    try {
      const p = new URLSearchParams();
      p.set("source", "live");
      p.set("upstream", name);   // ToolDetail: p.set("tool", name)
      if (cOutcome) p.set("outcome", cOutcome);
      if (cq) p.set("q", cq);
      p.set("limit", "20");
      const c = await getJSON(`/api/calls?${p}`);
      calls = c.items ?? [];
    } catch (_) { /* recent-calls is secondary; detail error UI owns errors */ }
  }
```
把现有 `load()` 里那一行 `const c = await getJSON(`/api/calls?source=live&upstream=...`); calls = c.items ?? [];` **删除**（改由 `loadCalls()` 负责），并在 `load()` 成功拿到 `d` 后调用 `loadCalls()`。再加一个 `$effect` 让过滤变化时重拉：
```js
  $effect(() => { void cOutcome; void cq; loadCalls(); });
```
> 注意：`loadCalls` 读 `name`；`load()`（详情主体）已有 3s 轮询，会顺带刷新 `calls`——也可让轮询调用 `loadCalls()`。保持 `name` 变化时 `load()` 重置、`cOutcome/cq` 变化时 `loadCalls()` 重拉即可。

在 `<h3>Recent calls</h3>` 之后、`<table>` 之前插入过滤行：
```svelte
  <div class="chips">
    {#each ["ok", "error", "timeout"] as o}
      <span class="chip" class:active={cOutcome === o} onclick={() => (cOutcome = cOutcome === o ? "" : o)}>{o}</span>
    {/each}
    <input class="search narrow" placeholder="search content…" bind:value={cq} />
  </div>
```

- [ ] **Step 2: 构建 + 内嵌断言 + 提交**

Run: `cd crates/dashboard/ui && npm run build && cd ../../.. && cargo test -p dashboard assets::`
Expected: 构建成功；`assets::` 3/3。
```bash
git add crates/dashboard/ui/src crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): outcome + content filters on detail Recent-calls lists

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4：e2e + 分层文档 + 四道门禁

**Files:**
- Modify: `crates/mcpgw/tests/dashboard.rs`（mock-上游 e2e 加内容过滤断言）
- Modify: `docs/L4-api/dashboard.md`、`docs/L3-details/dashboard.md`、`docs/L1-overview.md`

### e2e

- [ ] **Step 1: 扩 mock-上游 e2e**

在 `crates/mcpgw/tests/dashboard.rs` 的 `dashboard_detail_endpoints_with_mock_upstream`（已驱动 `call_tool mock__echo {text:"hi"}` 并已有 M1 内容捕获断言）里、`client.cancel()` 之前追加：
```rust
    // M2 content filters (live-only): free-text + structured arg key=value.
    let by_q: serde_json::Value = http
        .get(format!("{base}/api/calls?source=live&meta=call_tool&q=hi"))
        .send().await.unwrap().json().await.unwrap();
    assert!(by_q["total"].as_u64().unwrap() >= 1, "free-text q=hi matches the echo call");
    let by_arg: serde_json::Value = http
        .get(format!("{base}/api/calls?source=live&meta=call_tool&arg_key=text&arg_val=hi"))
        .send().await.unwrap().json().await.unwrap();
    assert!(by_arg["total"].as_u64().unwrap() >= 1, "arg_key=text arg_val=hi matches");
    let none: serde_json::Value = http
        .get(format!("{base}/api/calls?source=live&meta=call_tool&q=zzz_no_match_zzz"))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(none["total"].as_u64().unwrap(), 0, "non-matching q returns nothing");
```
Run（建 mock-stdio + 强制真跑）：`cargo build -p upstream --features testkit --bin mock-stdio && MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored`
Expected: 2 passed。

### 文档（READ 后改，忠实于代码）

- [ ] **Step 2: L4（`docs/L4-api/dashboard.md`）** —— `CallFilter`/`/api/calls` 段补内容过滤参数：`q`（自由文本，扫 args+result 子串）、`arg_key`+`arg_val`（结构化，递归找 args 里键=值），**仅 `source=live`**（history 项无内容、自动忽略）；`CallRingSink::query` 仅当存在内容过滤时才构建内容用于过滤、随后为列表剥离（列表始终不含内容）。`/api/calls` 路由行的查询参数补 `&q=&arg_key=&arg_val=`。
- [ ] **Step 3: L3（`docs/L3-details/dashboard.md`）** —— 「调用内容捕获」段补：内容过滤 `q`/`arg_key`+`arg_val` 只对 live（history 回放无内容、`matches` 的 `Some(args)` 门控自然忽略）；无内容过滤时 `query` 走轻量 `to_item(false)`、仅内容过滤时才付构建成本；`arg_key` 递归、截断/非法 JSON 不命中。测试覆盖补内容过滤单测 + e2e。
- [ ] **Step 4: L1（`docs/L1-overview.md`）** —— 调用内容捕获那条路线图末尾补一句：`并支持内容过滤（/api/calls 的 q 自由文本 + arg_key/arg_val 结构化，仅 live；主 Calls 页与详情页 Recent-calls 均有过滤 UI）`。测试计数行用 Step 5 实测回填。

### 门禁

- [ ] **Step 5: 四道门禁**

```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --locked
```
全绿。记录 `N passed / M ignored` 回填 L1；并 `cargo build -p upstream --features testkit --bin mock-stdio && MCPGW_REQUIRE_MOCK=1 cargo test -p mcpgw --test dashboard -- --ignored`（2 passed）；并 `cd crates/dashboard/ui && npm run build && cd ../../.. && git status --short crates/dashboard/ui/dist`（应为空）。

- [ ] **Step 6: 提交**

```bash
git add crates/mcpgw/tests/dashboard.rs docs/L1-overview.md docs/L3-details/dashboard.md docs/L4-api/dashboard.md
git commit -m "test+docs: M2 content-filter e2e + sync L1/L3/L4 (q + arg_key/arg_val)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## M2 完成判据（DoD）

- [ ] `/api/calls` 支持 `q`（args+result 子串）+ `arg_key`/`arg_val`（递归键=值），**仅 live**；history 自动忽略内容过滤。
- [ ] 无内容过滤时 `query` 仍轻量（`to_item(false)`，不 clone 内容）；有内容过滤时构建内容过滤、列表剥离内容（列表始终不含内容）。
- [ ] 主 Calls 页有内容搜索 + arg key/value 输入（history 禁用）；UpstreamDetail/ToolDetail 的 Recent-calls 有 outcome chips + 内容搜索。
- [ ] 四道门禁绿；mock e2e（q / arg_key 命中、非匹配为 0）实跑通过；dist 同步；L1/L3/L4 文档一致。

## 给实现者的备注
- **DRY**：内容过滤逻辑只在 `CallFilter::matches` + 两 helper；前端复用 `.search`/`.chip` 样式。
- **YAGNI**：只做 `q` + 单个 `arg_key=arg_val`；不做多键、不做正则、不做 history 内容过滤。
- **轻量路径**：务必保留「无内容过滤 → `to_item(false)`」，别让常规 3s 轮询付内容构建成本。
- **每改 ui/src 必 `npm run build` 并提交 dist**。
