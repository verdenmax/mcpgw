<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let q = $state("");
  let tools = $state([]);
  let error = $state(null);
  async function load() {
    try {
      const qs = q ? `?q=${encodeURIComponent(q)}` : "";
      tools = await getJSON(`/api/tools${qs}`); error = null;
    } catch (e) { error = String(e); }
  }
  // Refetch on every `q` change; no debounce needed — /api/tools is a cheap in-memory filter and
  // the 3s poll re-fetches with the current q, so any out-of-order keystroke result self-corrects.
  $effect(() => { void q; load(); });
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Tools</h2>
<input class="search" placeholder="filter tools…" bind:value={q} />
{#if error}<p class="error">{error}</p>{/if}
<p class="muted">{tools.length} tools</p>
<table>
  <thead><tr><th>name</th><th>description</th></tr></thead>
  <tbody>
    {#each tools as t}
      <tr><td>{t.name}</td><td>{t.description}</td></tr>
    {/each}
  </tbody>
</table>
