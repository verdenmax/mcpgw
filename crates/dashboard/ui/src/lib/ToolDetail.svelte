<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let { name } = $props();
  let d = $state(null);
  let calls = $state([]);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/tools/${encodeURIComponent(name)}`);
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/tools/${name} -> ${r.status}`);
      d = await r.json();
      const c = await getJSON(`/api/calls?source=live&tool=${encodeURIComponent(name)}&limit=20`);
      calls = c.items ?? [];
    } catch (e) { error = String(e); }
  }
  $effect(() => { name; load(); });
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
  function when(ms) { return new Date(ms).toLocaleString(); }
  function schema(v) { try { return JSON.stringify(v, null, 2); } catch (_) { return String(v); } }
</script>

<p><a href="#/tools">‹ back to Tools</a></p>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">tool not found</p>
{:else if d}
  <h2>{d.name}</h2>
  <p>upstream: <a href={`#/upstreams/${encodeURIComponent(d.server)}`}>{d.server}</a></p>
  <p>{d.description}</p>
  <h3>Input schema</h3>
  <pre class="schema">{schema(d.input_schema)}</pre>

  <h3>Recent calls</h3>
  <table>
    <thead><tr><th>time</th><th>meta</th><th>outcome</th><th>ms</th></tr></thead>
    <tbody>
      {#each calls as c}
        <tr class="row-link" onclick={() => (location.hash = `#/calls/${c.id}`)}>
          <td>{when(c.ts_unix_ms)}</td><td>{c.meta_tool}</td><td>{c.outcome}</td><td>{c.latency_ms}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
