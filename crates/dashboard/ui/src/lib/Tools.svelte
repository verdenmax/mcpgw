<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  import { go, rowKey } from "./format.js";
  import Icon from "./Icon.svelte";
  let q = $state("");
  let tools = $state(null);
  let error = $state(null);
  async function load() {
    const reqQ = q; // discard a superseded response if the filter changed mid-flight
    try {
      const qs = q ? `?q=${encodeURIComponent(q)}` : "";
      const t = await getJSON(`/api/tools${qs}`);
      if (reqQ !== q) return;
      tools = t; error = null;
    } catch (e) { if (reqQ === q) error = String(e); }
  }
  // Refetch on every `q` change; no debounce needed — /api/tools is a cheap in-memory filter and
  // the 3s poll re-fetches with the current q, so any out-of-order keystroke result self-corrects.
  $effect(() => { void q; load(); });
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Tools</h2>
<div class="toolbar">
  <input class="search" placeholder="filter tools…" bind:value={q} />
  {#if tools}<span class="meta-line" style="margin:0"><span class="count-pill">{tools.length}</span> match{tools.length === 1 ? "" : "es"}</span>{/if}
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if tools}
  {#if tools.length === 0}
    <div class="empty"><span class="ico"><Icon name="tools" size={28} /></span>
      <div>{q ? "No tools match this filter" : "No tools available"}</div>
      <div class="hint">{q ? "try a different query" : "connect an upstream that exposes tools"}</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>name</th><th>description</th></tr></thead>
      <tbody>
        {#each tools as t}
          {@const href = `#/tools/${encodeURIComponent(t.name)}`}
          <tr class="row-link" role="button" tabindex="0" onclick={() => go(href)} onkeydown={rowKey(href)}>
            <td class="mono">{t.name}</td><td>{t.description}</td>
          </tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(5) as _}<div class="sk row"></div>{/each}</div>
{/if}
