<script>
  import { onMount } from "svelte";
  let { id } = $props();
  let item = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    try {
      const r = await fetch(`/api/calls/${encodeURIComponent(id)}`);
      if (r.status === 404) { notFound = true; item = null; return; }
      if (!r.ok) throw new Error(`/api/calls/${id} -> ${r.status}`);
      item = await r.json(); error = null; notFound = false;
    } catch (e) { error = String(e); }
  }
  onMount(load);
  function when(ms) { return new Date(ms).toLocaleString(); }
</script>

<p><a href="#/calls">‹ back to Calls</a></p>
<h2>Call detail</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <p class="muted">call not found (it may have aged out of the live ring)</p>
{:else if item}
  <table>
    <tbody>
      <tr><th>id</th><td>{item.id}</td></tr>
      <tr><th>time</th><td>{when(item.ts_unix_ms)}</td></tr>
      <tr><th>meta_tool</th><td>{item.meta_tool}</td></tr>
      <tr><th>target_tool</th><td>{#if item.target_tool}<a href="#/tools">{item.target_tool}</a>{:else}—{/if}</td></tr>
      <tr><th>upstream</th><td>{#if item.upstream}<a href="#/upstreams">{item.upstream}</a>{:else}—{/if}</td></tr>
      <tr><th>outcome</th><td>{item.outcome}</td></tr>
      <tr><th>error_kind</th><td>{item.error_kind ?? "—"}</td></tr>
      <tr><th>latency_ms</th><td>{item.latency_ms}</td></tr>
      <tr><th>arg_bytes</th><td>{item.arg_bytes}</td></tr>
      <tr><th>result_bytes</th><td>{item.result_bytes}</td></tr>
    </tbody>
  </table>
{:else}
  <p class="muted">loading…</p>
{/if}
