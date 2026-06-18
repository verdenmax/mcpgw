<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let { name } = $props();
  let d = $state(null);
  let calls = $state([]);
  let cOutcome = $state("");  // recent-calls outcome filter
  let cq = $state("");        // recent-calls content search
  let error = $state(null);
  let notFound = $state(false);
  async function loadCalls() {
    try {
      const p = new URLSearchParams();
      p.set("source", "live");
      p.set("tool", name);
      if (cOutcome) p.set("outcome", cOutcome);
      if (cq) p.set("q", cq);
      p.set("limit", "20");
      const c = await getJSON(`/api/calls?${p}`);
      calls = c.items ?? [];
    } catch (_) { /* recent-calls is secondary; detail error UI owns errors */ }
  }
  async function load() {
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/tools/${encodeURIComponent(name)}`);
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/tools/${name} -> ${r.status}`);
      d = await r.json();
      await loadCalls();
    } catch (e) { error = String(e); }
  }
  $effect(() => { name; load(); });
  $effect(() => { void cOutcome; void cq; loadCalls(); });
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
  {#if d.input_schema === null || d.input_schema === undefined}
    <p class="muted">(no schema)</p>
  {:else}
    <pre class="schema">{schema(d.input_schema)}</pre>
  {/if}

  <h3>Recent calls</h3>
  <div class="chips">
    {#each ["ok", "error", "timeout"] as o}
      <span class="chip" class:active={cOutcome === o} onclick={() => (cOutcome = cOutcome === o ? "" : o)}>{o}</span>
    {/each}
    <input class="search narrow" placeholder="search content…" bind:value={cq} />
  </div>
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
