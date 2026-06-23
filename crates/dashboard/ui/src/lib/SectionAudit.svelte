<script>
  import { defaultSection } from "./configSchema.js";
  let { audit = $bindable() } = $props();
</script>

{#if audit === undefined}
  <button type="button" class="admbtn" onclick={() => (audit = defaultSection("audit"))}>+ 启用 [audit]</button>
{:else}
  <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={audit.enabled} /></label>
  <label class="cfg-field">path <input bind:value={audit.path} /></label>
{/if}
