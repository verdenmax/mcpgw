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
      const r = await fetch(`/api/upstreams/${encodeURIComponent(name)}`);
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/upstreams/${name} -> ${r.status}`);
      d = await r.json();
      const c = await getJSON(`/api/calls?source=live&upstream=${encodeURIComponent(name)}&limit=20`);
      calls = c.items ?? [];
    } catch (e) { error = String(e); }
  }
  $effect(() => { name; load(); });          // reload when the upstream name changes
  onMount(() => {                              // poll mutable data (calls/errors/recent calls)
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  });
  function when(ms) { return new Date(ms).toLocaleString(); }
</script>

<p><a href="#/upstreams">‹ back to Upstreams</a></p>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">upstream not found</p>
{:else if d}
  <h2>{d.name}</h2>
  <div class="cards">
    <div class="card"><div class="label">transport</div><div class="v">{d.transport}</div></div>
    <div class="card"><div class="label">status</div><div class="v"><span class="badge {d.status}">{d.status}</span></div></div>
    <div class="card"><div class="label">tools</div><div class="v">{d.tools_count}</div></div>
    <div class="card"><div class="label">calls</div><div class="v">{d.calls}</div></div>
    <div class="card"><div class="label">errors</div><div class="v">{d.errors}</div></div>
  </div>
  {#if d.reason}<p class="muted">reason: {d.reason}</p>{/if}

  <h3>Tools</h3>
  <table>
    <thead><tr><th>name</th><th>description</th></tr></thead>
    <tbody>
      {#each d.tools as t}
        <tr class="row-link" onclick={() => (location.hash = `#/tools/${encodeURIComponent(t.name)}`)}>
          <td>{t.name}</td><td>{t.description}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  <h3>Recent calls</h3>
  <table>
    <thead><tr><th>time</th><th>meta</th><th>target</th><th>outcome</th><th>ms</th></tr></thead>
    <tbody>
      {#each calls as c}
        <tr class="row-link" onclick={() => (location.hash = `#/calls/${c.id}`)}>
          <td>{when(c.ts_unix_ms)}</td><td>{c.meta_tool}</td><td>{c.target_tool ?? "—"}</td><td>{c.outcome}</td><td>{c.latency_ms}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
