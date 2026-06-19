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
      p.set("upstream", name);
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
      const r = await fetch(`/api/upstreams/${encodeURIComponent(name)}`);
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/upstreams/${name} -> ${r.status}`);
      d = await r.json();
      await loadCalls();
    } catch (e) { error = String(e); }
  }
  $effect(() => { name; load(); });          // reload when the upstream name changes
  $effect(() => { void cOutcome; void cq; loadCalls(); });
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
  <div class="chips">
    {#each ["ok", "error", "timeout"] as o}
      <span class="chip" class:active={cOutcome === o} onclick={() => (cOutcome = cOutcome === o ? "" : o)}>{o}</span>
    {/each}
    <input class="search narrow" placeholder="search content…" bind:value={cq} />
  </div>
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
