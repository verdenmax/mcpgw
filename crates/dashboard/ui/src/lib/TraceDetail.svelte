<script>
  let { id } = $props();
  let t = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/traces/${encodeURIComponent(id)}`);
      if (r.status === 404) { notFound = true; t = null; return; }
      if (!r.ok) throw new Error(`/api/traces/${id} -> ${r.status}`);
      t = await r.json();
    } catch (e) { error = String(e); }
  }
  $effect(() => { id; load(); });
  function when(ms) { return new Date(ms).toLocaleString(); }
</script>

<p><a href="#/traces">‹ back to Traces</a></p>
<h2>Trace detail</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">trace not found (it may have aged out of the live ring)</p>
{:else if t}
  <table>
    <tbody>
      <tr><th>id</th><td>{t.id}</td></tr>
      <tr><th>time</th><td>{when(t.ts_unix_ms)}</td></tr>
      <tr><th>query</th><td>{t.query}</td></tr>
      <tr><th>top_k</th><td>{t.top_k}</td></tr>
      <tr><th>latency_ms</th><td>{t.latency_ms}</td></tr>
    </tbody>
  </table>
  <h3>Hits ({t.results.length})</h3>
  <table>
    <thead><tr><th>tool</th><th>score</th></tr></thead>
    <tbody>
      {#each t.results as h}
        <tr class="row-link" onclick={() => (location.hash = `#/tools/${encodeURIComponent(h.name)}`)}>
          <td>{h.name}</td><td>{h.score.toFixed(3)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
