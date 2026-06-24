<script>
  import { defaultSection } from "./configSchema.js";
  let { dashboard = $bindable() } = $props();
</script>

{#if dashboard === undefined}
  <button type="button" class="iconbtn" onclick={() => (dashboard = defaultSection("dashboard"))}>+ 启用 [dashboard]</button>
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={dashboard.enabled} /><span class="cfg-hint">是否开启可视化面板</span></label>
  <label class="cfg-field">bind
    <input bind:value={dashboard.bind} />
    <span class="cfg-hint">面板监听地址，host:port，如 127.0.0.1:8971</span>
  </label>
  <label class="cfg-field cfg-switch">trace_queries <input type="checkbox" bind:checked={dashboard.trace_queries} /><span class="cfg-hint">是否捕获 query→tools 的检索追踪（供面板回放）</span></label>
  <label class="cfg-field"><span class="cfg-lbl">trace_path <span class="cfg-q" title="检索追踪 JSONL 路径（配了才有「历史」回放，可选）" aria-label="检索追踪 JSONL 路径（配了才有「历史」回放，可选）">?</span></span><input bind:value={dashboard.trace_path} placeholder="(可选)" /></label>
  <label class="cfg-field"><span class="cfg-lbl">trace_buffer <span class="cfg-q" title="内存中保留的检索追踪条数" aria-label="内存中保留的检索追踪条数">?</span></span><input type="number" min="1" bind:value={dashboard.trace_buffer} /></label>
  <label class="cfg-field"><span class="cfg-lbl">call_buffer <span class="cfg-q" title="内存中保留的调用记录条数" aria-label="内存中保留的调用记录条数">?</span></span><input type="number" min="1" bind:value={dashboard.call_buffer} /></label>
  <label class="cfg-field"><span class="cfg-lbl">payload_max_bytes <span class="cfg-q" title="单条调用 args/result 入环的字节上限" aria-label="单条调用 args/result 入环的字节上限">?</span></span><input type="number" min="1" bind:value={dashboard.payload_max_bytes} /></label>
  <label class="cfg-field"><span class="cfg-lbl">admin_token_env <span class="cfg-q" title="admin 写操作 Bearer token 的环境变量名（不配则写 API 全 404，可选）" aria-label="admin 写操作 Bearer token 的环境变量名（不配则写 API 全 404，可选）">?</span></span><input bind:value={dashboard.admin_token_env} placeholder="环境变量名(可选)" /></label>
  <label class="cfg-field"><span class="cfg-lbl">disabled_state_path <span class="cfg-q" title="运行时禁用集的持久化文件路径（可选）" aria-label="运行时禁用集的持久化文件路径（可选）">?</span></span><input bind:value={dashboard.disabled_state_path} placeholder="(可选)" /></label>
{/if}
