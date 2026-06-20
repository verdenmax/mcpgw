<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  import { go } from "./format.js";
  import Icon from "./Icon.svelte";
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
  <button class="chip" class:active={source === "live"} onclick={() => (source = "live")}>live</button>
  <button class="chip" class:active={source === "history"} onclick={() => (source = "history")}>history</button>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if resp}
  {#if resp.history_unavailable}
    <div class="empty"><span class="ico"><Icon name="traces" size={28} /></span>
      <div>History unavailable</div>
      <div class="hint">enable <code>[dashboard].trace_path</code> to persist traces</div></div>
  {:else if resp.traces.length === 0}
    <div class="empty"><span class="ico"><Icon name="traces" size={28} /></span>
      <div>No {source} traces yet</div>
      <div class="hint">run a <code>search_tools</code> query to capture one</div></div>
  {:else}
    {#each resp.traces as t}
      <button class="trace-card" onclick={() => go(`#/traces/${t.id}`)}>
        <div class="q">{t.query}</div>
        <div>{#each t.results as h}<span class="tag">{h.name} <span class="score">{h.score.toFixed(2)}</span></span>{/each}
          {#if t.results.length === 0}<span class="muted">no hits</span>{/if}</div>
      </button>
    {/each}
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(4) as _}<div class="sk card"></div>{/each}</div>
{/if}
