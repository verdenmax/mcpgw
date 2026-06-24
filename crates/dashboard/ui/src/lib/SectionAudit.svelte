<script>
  import { defaultSection } from "./configSchema.js";
  let { audit = $bindable() } = $props();
</script>

{#if audit === undefined}
  <button type="button" class="iconbtn" onclick={() => (audit = defaultSection("audit"))}>+ 启用 [audit]</button>
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={audit.enabled} /><span class="cfg-hint">是否开启调用审计（落 JSONL）</span></label>
  <label class="cfg-field">path
    <input bind:value={audit.path} />
    <span class="cfg-hint">审计 JSONL 文件路径</span>
  </label>
{/if}
