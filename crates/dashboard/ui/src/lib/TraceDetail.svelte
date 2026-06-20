<script>
  import { go, rowKey, when } from "./format.js";
  import Icon from "./Icon.svelte";
  let { id } = $props();
  let t = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    const reqId = id; // capture: ignore this response if `id` changes before it resolves
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/traces/${encodeURIComponent(id)}`);
      if (reqId !== id) return; // a newer id superseded this request
      if (r.status === 404) { notFound = true; t = null; return; }
      if (!r.ok) throw new Error(`/api/traces/${id} -> ${r.status}`);
      const next = await r.json();
      if (reqId !== id) return;
      t = next;
    } catch (e) { if (reqId === id) error = String(e); }
  }
  $effect(() => { id; load(); });
</script>

<a class="back" href="#/traces"><Icon name="back" size={14} /> Traces</a>
<h2>Trace detail</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notFound}
  <div class="empty"><span class="ico"><Icon name="traces" size={28} /></span>
    <div>Trace not found</div><div class="hint">it may have aged out of the live ring</div></div>
{:else if t}
  <div class="table-wrap"><table class="kv">
    <tbody>
      <tr><th>id</th><td>{t.id}</td></tr>
      <tr><th>time</th><td>{when(t.ts_unix_ms)}</td></tr>
      <tr><th>query</th><td>{t.query}</td></tr>
      <tr><th>top_k</th><td>{t.top_k}</td></tr>
      <tr><th>latency_ms</th><td>{t.latency_ms}</td></tr>
    </tbody>
  </table></div>
  <h3>Hits ({t.results.length})</h3>
  {#if t.results.length === 0}
    <div class="empty"><div>No hits for this query</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>tool</th><th class="num">score</th></tr></thead>
      <tbody>
        {#each t.results as h}
          {@const href = `#/tools/${encodeURIComponent(h.name)}`}
          <tr class="row-link" role="button" tabindex="0" onclick={() => go(href)} onkeydown={rowKey(href)}>
            <td class="mono">{h.name}</td><td class="num">{h.score.toFixed(3)}</td>
          </tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}
{:else}
  <div class="skeleton">{#each Array(3) as _}<div class="sk row"></div>{/each}</div>
{/if}
