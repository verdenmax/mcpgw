# Dashboard Config 字段说明 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 dashboard Config 表单全部 5 段的每个字段加说明，混用 inline 小灰字（必填/关键）+ `?` hover tooltip（可选/技术细节），逻辑零改动。

**Architecture:** 纯前端文案/视觉添加，全部在 `crates/dashboard/ui`。各 `Section*.svelte` 字段 `<label class="cfg-field">` 内按 spec 逐字段表追加 inline `<span class="cfg-hint">` 或 `?` `<span class="cfg-q">`；`app.css` 加 `.cfg-hint`/`.cfg-q` 规则（mock 已验证）。校验/同步/Save/`configSchema`/后端**不变**——现有 28 vitest 必须保持全绿。

**Tech Stack:** Svelte 5（runes）、Vite、全局 `app.css`。文案见 spec `2026-06-24-mcpgw-dashboard-config-field-help-design.md` 第 4 节逐字段表。

---

## 两种写法模式（所有 Section 统一遵循）

- **inline**：在控件**之后**追加（`.cfg-field` 是 flex column，hint 自然落到控件下一行）：
  ```svelte
  <label class="cfg-field">字段名
    <input ... />
    <span class="cfg-hint">说明文案</span>
  </label>
  ```
- **tooltip**：在字段名**之后、控件之前**追加 `?`（原生 `title` 即 hover 提示 + `aria-label` 供 AT）：
  ```svelte
  <label class="cfg-field">字段名 <span class="cfg-q" title="说明文案" aria-label="说明文案">?</span><input ... /></label>
  ```
- **只加 `<span>` 元素**，绝不改 `bind:`/`onchange`/`oninput`/`$effect`/逻辑。

## 视觉任务的验证方式（贯穿所有 task）

无新单测（逻辑零改动）。每 task：① `npm run test` 仍 **28 passed**（回归）；② `npm run build` exit 0；③ 重建并提交 `dist/`；④ 视觉手测（demo 看该段每字段有说明、inline/tooltip 符合 spec、`?` hover 出提示）。

## ⚠️ 实施前置：还原 mock

工作区当前有未提交的 retrieval mock（`SectionRetrieval.svelte` + `app.css` + `dist`）。**开始前先还原干净**，正式实现以 spec 表为准（mock 的 vector 字段是全 tooltip，正式版 `model`/`api_key_env` 应为 inline）：
```bash
git checkout crates/dashboard/ui/src/lib/SectionRetrieval.svelte crates/dashboard/ui/src/app.css crates/dashboard/ui/dist
git clean -fd crates/dashboard/ui/dist
```
（这在 subagent-driven 的「开 feature 分支」之前由控制者完成，subagent 拿到的是干净树。）

---

## File Structure

全部在 `crates/dashboard/ui/`：

| 文件 | 改动 | Task |
| --- | --- | --- |
| `src/app.css` | 加 `.cfg-hint` + `.cfg-q` 规则 | 1 |
| `src/lib/SectionRetrieval.svelte` | strategy/top_k/model/api_key_env inline；base_url/dim/timeout_ms/batch_size/candidates tooltip | 1 |
| `src/lib/SectionServer.svelte` | stdio/http.enabled/bind/api_key.name/env inline；path tooltip | 2 |
| `src/lib/SectionAudit.svelte` | enabled/path inline | 2 |
| `src/lib/SectionDashboard.svelte` | enabled/bind/trace_queries inline；trace_path/buffers/admin_token_env/disabled_state_path tooltip | 2 |
| `src/lib/SectionUpstreams.svelte` | name/transport/command/url inline；call_timeout_ms/args/env_passthrough/bearer_env/headers tooltip | 3 |
| `dist/` | 每个改组件 task 重建并提交 | 1–3 |

---

## Task 1: `app.css` 样式 + retrieval 段（建立模式）

**Files:**
- Modify: `crates/dashboard/ui/src/app.css`
- Modify: `crates/dashboard/ui/src/lib/SectionRetrieval.svelte`
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: `app.css` 加 hint/tooltip 样式**

在 `app.css` 的 `.cfg-errs code { … }` 规则之后追加：
```css
/* field help: inline hint + ? tooltip */
.cfg-hint { font-size: var(--fs-2xs); color: var(--muted); line-height: 1.45; }
.cfg-field.cfg-switch { flex-wrap: wrap; }
.cfg-field.cfg-switch .cfg-hint { flex-basis: 100%; }
.cfg-q { display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px;
  border-radius: 50%; background: var(--panel); border: 1px solid var(--border); color: var(--muted);
  font-size: 10px; cursor: help; margin-left: 4px; vertical-align: middle;
  transition: color .14s, border-color .14s; }
.cfg-q:hover { color: var(--fg); border-color: var(--border-hover); }
```
确认 `--fs-2xs`/`--muted`/`--panel`/`--border`/`--fg`/`--border-hover` 均在 `:root`（grep）。

- [ ] **Step 2: 重写 `SectionRetrieval.svelte` 的 `{:else}` 字段块**

把 `{:else}` 到该组件结尾之间的字段部分（strategy/top_k + vector/subagent 子表）替换为（保留 `<script>`、`{#if retrieval === undefined}` 分支、`role="group"`/`cfg-sub-h` 不变）：
```svelte
{:else}
  <label class="cfg-field">strategy
    <select bind:value={retrieval.strategy} onchange={onStrategy}>{#each STRATEGIES as s}<option value={s}>{s}</option>{/each}</select>
    <span class="cfg-hint">检索策略：bm25=纯词法召回（无需 key）、vector=向量语义、hybrid=词法+向量混合、subagent=智能体规划</span>
  </label>
  <label class="cfg-field">top_k
    <input type="number" min="1" bind:value={retrieval.top_k} />
    <span class="cfg-hint">每次检索返回给客户端的工具条数上限</span>
  </label>

  {#if (retrieval.strategy === "vector" || retrieval.strategy === "hybrid") && retrieval.vector}
    <div class="cfg-sub" role="group" aria-label="vector">
      <div class="cfg-sub-h">vector</div>
      <label class="cfg-field">model
        <input bind:value={retrieval.vector.model} />
        <span class="cfg-hint">向量化（embedding）模型名，如 text-embedding-3-small</span>
      </label>
      <label class="cfg-field">api_key_env
        <input bind:value={retrieval.vector.api_key_env} placeholder="环境变量名" />
        <span class="cfg-hint">存放 API key 的环境变量名（只填变量名，不填密钥本身）</span>
      </label>
      <label class="cfg-field">base_url <span class="cfg-q" title="向量化服务的 API 基地址；留空用内置默认" aria-label="向量化服务的 API 基地址；留空用内置默认">?</span><input bind:value={retrieval.vector.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">dim <span class="cfg-q" title="向量维度，需与所选模型匹配（可选）" aria-label="向量维度，需与所选模型匹配（可选）">?</span><input type="number" min="1" bind:value={retrieval.vector.dim} /></label>
      <label class="cfg-field">timeout_ms <span class="cfg-q" title="单次向量化请求的超时（毫秒）" aria-label="单次向量化请求的超时（毫秒）">?</span><input type="number" min="1" bind:value={retrieval.vector.timeout_ms} /></label>
      <label class="cfg-field">batch_size <span class="cfg-q" title="批量向量化时每批的条数" aria-label="批量向量化时每批的条数">?</span><input type="number" min="1" bind:value={retrieval.vector.batch_size} /></label>
    </div>
  {/if}
  {#if retrieval.strategy === "subagent" && retrieval.subagent}
    <div class="cfg-sub" role="group" aria-label="subagent">
      <div class="cfg-sub-h">subagent</div>
      <label class="cfg-field">model
        <input bind:value={retrieval.subagent.model} />
        <span class="cfg-hint">规划用 LLM 模型名</span>
      </label>
      <label class="cfg-field">api_key_env
        <input bind:value={retrieval.subagent.api_key_env} placeholder="环境变量名" />
        <span class="cfg-hint">存放 API key 的环境变量名（只填变量名）</span>
      </label>
      <label class="cfg-field">base_url <span class="cfg-q" title="LLM 服务 API 基地址；留空用默认" aria-label="LLM 服务 API 基地址；留空用默认">?</span><input bind:value={retrieval.subagent.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">timeout_ms <span class="cfg-q" title="单次规划请求超时（毫秒）" aria-label="单次规划请求超时（毫秒）">?</span><input type="number" min="1" bind:value={retrieval.subagent.timeout_ms} /></label>
      <label class="cfg-field">candidates <span class="cfg-q" title="每轮候选工具数（可选）" aria-label="每轮候选工具数（可选）">?</span><input type="number" min="1" bind:value={retrieval.subagent.candidates} /></label>
    </div>
  {/if}
{/if}
```
（注意：vector/subagent 子表内字段顺序调整为「inline 必填项在前、tooltip 可选项在后」，更易读；所有 `bind:value` 目标与原一致。）

- [ ] **Step 3: build + 回归测试**

Run: `cd crates/dashboard/ui && npm run test` → **28 passed**. Run: `npm run build` → exit 0. After staging, fresh build leaves `git status --porcelain crates/dashboard/ui/dist` empty.

- [ ] **Step 4: 视觉手测**

Retrieval 段：strategy/top_k、以及 vector/subagent 的 model/api_key_env 下方有常驻小灰字说明；base_url/dim/timeout_ms/batch_size/candidates 字段名后有 `?`，hover 出提示。

- [ ] **Step 5: Commit（含 dist）**

```bash
git add crates/dashboard/ui/src/app.css crates/dashboard/ui/src/lib/SectionRetrieval.svelte crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): Config 字段说明——样式 + retrieval 段（inline+tooltip）"
```

> 注：switch 字段（`cfg-switch`）的 inline hint 借 `flex-wrap:wrap` + `flex-basis:100%` 换行到开关行下方；数组项（`api_key`/`headers` 这类「数组 of 小对象」）无单独控件位放 inline，统一在数组的 `<span class="label">` 处加**数组级 `?` 说明**描述其字段含义。

---

## Task 2: server + audit + dashboard 段

**Files:**
- Modify: `crates/dashboard/ui/src/lib/SectionServer.svelte`
- Modify: `crates/dashboard/ui/src/lib/SectionAudit.svelte`
- Modify: `crates/dashboard/ui/src/lib/SectionDashboard.svelte`
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: `SectionServer.svelte` — 替换 `{:else}` 块**

把 `{:else}` 到组件结尾的字段部分替换为（`<script>`、`{#if server === undefined}` 分支、`enableHttp`/`addKey`/`rmKey` 不变）：
```svelte
{:else}
  <label class="cfg-field cfg-switch">stdio <input type="checkbox" bind:checked={server.stdio} /><span class="cfg-hint">是否开启 stdio 传输（供本地 MCP 客户端经标准输入输出连接）</span></label>
  {#if !server.http}
    <button type="button" class="iconbtn" onclick={enableHttp}>+ 启用 [server.http]</button>
  {:else}
    <div class="cfg-sub" role="group" aria-label="http">
      <div class="cfg-sub-h">http</div>
      <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={server.http.enabled} /><span class="cfg-hint">是否开启 HTTP（Streamable HTTP）传输</span></label>
      <label class="cfg-field">bind
        <input bind:value={server.http.bind} />
        <span class="cfg-hint">HTTP 监听地址，host:port，如 127.0.0.1:8970</span>
      </label>
      <label class="cfg-field">path <span class="cfg-q" title="MCP 端点路径，默认 /mcp" aria-label="MCP 端点路径，默认 /mcp">?</span><input bind:value={server.http.path} /></label>
      <div class="cfg-arr"><span class="label">api_key <span class="cfg-q" title="每条：name=key 标签（仅日志/观测，非密钥本身）、env=存放该 key 的环境变量名" aria-label="每条：name=key 标签（仅日志/观测，非密钥本身）、env=存放该 key 的环境变量名">?</span></span>
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
  {/if}
{/if}
```

- [ ] **Step 2: `SectionAudit.svelte` — 替换 `{:else}` 块**

```svelte
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={audit.enabled} /><span class="cfg-hint">是否开启调用审计（落 JSONL）</span></label>
  <label class="cfg-field">path
    <input bind:value={audit.path} />
    <span class="cfg-hint">审计 JSONL 文件路径</span>
  </label>
{/if}
```

- [ ] **Step 3: `SectionDashboard.svelte` — 替换 `{:else}` 块**

```svelte
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={dashboard.enabled} /><span class="cfg-hint">是否开启可视化面板</span></label>
  <label class="cfg-field">bind
    <input bind:value={dashboard.bind} />
    <span class="cfg-hint">面板监听地址，host:port，如 127.0.0.1:8971</span>
  </label>
  <label class="cfg-field cfg-switch">trace_queries <input type="checkbox" bind:checked={dashboard.trace_queries} /><span class="cfg-hint">是否捕获 query→tools 的检索追踪（供面板回放）</span></label>
  <label class="cfg-field">trace_path <span class="cfg-q" title="检索追踪 JSONL 路径（配了才有「历史」回放，可选）" aria-label="检索追踪 JSONL 路径（配了才有「历史」回放，可选）">?</span><input bind:value={dashboard.trace_path} placeholder="(可选)" /></label>
  <label class="cfg-field">trace_buffer <span class="cfg-q" title="内存中保留的检索追踪条数" aria-label="内存中保留的检索追踪条数">?</span><input type="number" min="1" bind:value={dashboard.trace_buffer} /></label>
  <label class="cfg-field">call_buffer <span class="cfg-q" title="内存中保留的调用记录条数" aria-label="内存中保留的调用记录条数">?</span><input type="number" min="1" bind:value={dashboard.call_buffer} /></label>
  <label class="cfg-field">payload_max_bytes <span class="cfg-q" title="单条调用 args/result 入环的字节上限" aria-label="单条调用 args/result 入环的字节上限">?</span><input type="number" min="1" bind:value={dashboard.payload_max_bytes} /></label>
  <label class="cfg-field">admin_token_env <span class="cfg-q" title="admin 写操作 Bearer token 的环境变量名（不配则写 API 全 404，可选）" aria-label="admin 写操作 Bearer token 的环境变量名（不配则写 API 全 404，可选）">?</span><input bind:value={dashboard.admin_token_env} placeholder="环境变量名(可选)" /></label>
  <label class="cfg-field">disabled_state_path <span class="cfg-q" title="运行时禁用集的持久化文件路径（可选）" aria-label="运行时禁用集的持久化文件路径（可选）">?</span><input bind:value={dashboard.disabled_state_path} placeholder="(可选)" /></label>
{/if}
```

- [ ] **Step 4: build + 回归测试**

Run: `cd crates/dashboard/ui && npm run test` → **28 passed**. Run: `npm run build` → exit 0. After staging, fresh build leaves `git status --porcelain crates/dashboard/ui/dist` empty.

- [ ] **Step 5: 视觉手测**

Server/Audit/Dashboard 段：开关类（stdio/enabled/trace_queries）说明换行在开关下方；bind/path/api_key 等按 spec 呈现 inline 或 `?`；hover `?` 出提示；功能不变。

- [ ] **Step 6: Commit（含 dist）**

```bash
git add crates/dashboard/ui/src/lib/SectionServer.svelte crates/dashboard/ui/src/lib/SectionAudit.svelte crates/dashboard/ui/src/lib/SectionDashboard.svelte crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): Config 字段说明——server/audit/dashboard 段"
```

---

## Task 3: upstream 段

**Files:**
- Modify: `crates/dashboard/ui/src/lib/SectionUpstreams.svelte`
- Rebuild: `crates/dashboard/ui/dist/`

- [ ] **Step 1: 替换 `{#each upstream …}` 内的 `<div class="cfg-sub cfg-upstream" …>` 字段块**

把每个 upstream 条目的字段部分替换为（`<script>`、`{#if !upstream || …}` 空态、`add`/`remove`/`onTransport`/`addHeader`/`setHeaderKey`/`rmHeader`、外层 `{#each}` 与 `+ add upstream` 按钮、`cfg-sub-h`(含 remove) 不变）：
```svelte
  <div class="cfg-sub" role="group" aria-label={`upstream ${i}`}>
    <div class="cfg-sub-h">upstream[{i}] <button type="button" class="iconbtn" onclick={() => remove(i)}>✕ 移除</button></div>
    <label class="cfg-field">name
      <input bind:value={u.name} placeholder="唯一、非空、不含 __" />
      <span class="cfg-hint">该上游工具的命名空间前缀；非空、唯一、不含 __</span>
    </label>
    <label class="cfg-field">transport
      <select bind:value={u.transport} onchange={() => onTransport(u)}>
        {#each TRANSPORTS as t}<option value={t}>{t}</option>{/each}
      </select>
      <span class="cfg-hint">连接方式：stdio=本地子进程、http=远程 Streamable HTTP</span>
    </label>
    <label class="cfg-field">call_timeout_ms <span class="cfg-q" title="单次工具调用超时（毫秒，默认 30000）" aria-label="单次工具调用超时（毫秒，默认 30000）">?</span><input type="number" min="1" bind:value={u.call_timeout_ms} /></label>
    {#if u.transport === "stdio"}
      <label class="cfg-field">command
        <input bind:value={u.command} placeholder="可执行路径" />
        <span class="cfg-hint">子进程可执行文件路径</span>
      </label>
      <label class="cfg-field">args <span class="cfg-q" title="子进程启动参数（空格分隔；含空格的参数请用 raw 模式）" aria-label="子进程启动参数（空格分隔；含空格的参数请用 raw 模式）">?</span><input value={(u.args ?? []).join(" ")} oninput={(e) => (u.args = e.target.value.split(/\s+/).filter(Boolean))} placeholder="空格分隔" /></label>
      <label class="cfg-field">env_passthrough <span class="cfg-q" title="透传给子进程的环境变量名（其余环境被清空）" aria-label="透传给子进程的环境变量名（其余环境被清空）">?</span><input value={(u.env_passthrough ?? []).join(" ")} oninput={(e) => (u.env_passthrough = e.target.value.split(/\s+/).filter(Boolean))} placeholder="如 PATH HOME" /></label>
    {:else if u.transport === "http"}
      <label class="cfg-field">url
        <input bind:value={u.url} placeholder="https://…/mcp" />
        <span class="cfg-hint">远程 MCP 端点 URL，如 https://…/mcp</span>
      </label>
      <label class="cfg-field">bearer_env <span class="cfg-q" title="存放 Bearer token 的环境变量名（→ Authorization: Bearer，可选）" aria-label="存放 Bearer token 的环境变量名（→ Authorization: Bearer，可选）">?</span><input bind:value={u.bearer_env} placeholder="环境变量名(可选)" /></label>
      <div class="cfg-arr"><span class="label">headers <span class="cfg-q" title="自定义请求头：header 名 → 存放其值的环境变量名" aria-label="自定义请求头：header 名 → 存放其值的环境变量名">?</span></span>
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
（字段顺序：把 inline 必填项 name/transport/command/url 排前，tooltip 可选项随后。所有 `bind:`/`oninput`/`onchange` 与原一致。）

- [ ] **Step 2: build + 回归测试**

Run: `cd crates/dashboard/ui && npm run test` → **28 passed**. Run: `npm run build` → exit 0. `git status --porcelain crates/dashboard/ui/dist` empty after staging.

- [ ] **Step 3: 视觉手测**

Upstreams 段：增 1 个 upstream；name/transport inline 说明常驻；call_timeout_ms/args/env_passthrough/bearer_env/headers 是 `?`；切 stdio↔http 字段+说明随之切换；功能不变。

- [ ] **Step 4: Commit（含 dist）**

```bash
git add crates/dashboard/ui/src/lib/SectionUpstreams.svelte crates/dashboard/ui/dist
git commit -m "feat(dashboard/ui): Config 字段说明——upstream 段"
```

---

## Task 4: 最终验收

- [ ] **Step 1: 前端回归 + dist 可复现**

```
cd crates/dashboard/ui && npm run test          # 28 passed（逻辑零改动）
rm -rf dist && npm ci && npm run build
cd ../../.. && git status --porcelain crates/dashboard/ui/dist   # MUST be empty
```

- [ ] **Step 2: 后端不受影响**

```
cargo build --locked      # rust-embed 嵌入新 dist
cargo test --all-features # 仍 328 passed（前端-only）
```

- [ ] **Step 3: demo 端到端手测**

重启 demo，浏览器 Config→Form 逐段核对：5 段每字段都有说明；inline（必填/关键，含 switch 换行）与 `?` tooltip（可选/技术细节、数组项数组级）按 spec 划分；hover `?` 出提示；Raw↔Form 切换、strategy 切换子表、upstream 增删与 transport 切换、校验红框、Save 热重载等功能零回归。

## 完成标准

- 3 个实现 task + 验收全部提交；`npm run test` 28 passed、`dist` 字节级可复现；后端 `cargo build`/`test` 不受影响；demo 每字段有说明、形式符合 spec、功能零回归。
