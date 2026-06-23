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
  <label class="cfg-field">trace_buffer <input type="number" min="1" bind:value={dashboard.trace_buffer} /></label>
  <label class="cfg-field">call_buffer <input type="number" min="1" bind:value={dashboard.call_buffer} /></label>
  <label class="cfg-field">payload_max_bytes <input type="number" min="1" bind:value={dashboard.payload_max_bytes} /></label>
  <label class="cfg-field">admin_token_env <input bind:value={dashboard.admin_token_env} placeholder="环境变量名(可选)" /></label>
  <label class="cfg-field">disabled_state_path <input bind:value={dashboard.disabled_state_path} placeholder="(可选)" /></label>
{/if}
