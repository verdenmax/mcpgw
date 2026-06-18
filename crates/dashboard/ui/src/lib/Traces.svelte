<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let source = $state("live");
  let resp = $state(null);
  let error = $state(null);
  async function load() {
    try { resp = await getJSON(`/api/traces?limit=50&source=${source}`); error = null; }
    catch (e) { error = String(e); }
  }
  $effect(() => { void source; load(); });
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Query traces</h2>
<div class="chips">
  <span class="chip" class:active={source === "live"} onclick={() => (source = "live")}>live</span>
  <span class="chip" class:active={source === "history"} onclick={() => (source = "history")}>history</span>
</div>
{#if error}<p class="error">{error}</p>{/if}
{#if resp}
  {#if resp.history_unavailable}
    <p class="muted">history unavailable (enable [dashboard].trace_path)</p>
  {:else}
    {#each resp.traces as t}
      <div class="card trace-card">
        <div class="label">{t.query}</div>
        <div>{#each t.results as h}<span class="chip">{h.name} ({h.score.toFixed(2)})</span> {/each}</div>
      </div>
    {/each}
  {/if}
{:else if !error}
  <p class="muted">loading…</p>
{/if}
