<script>
  import { when, pretty } from "./format.js";
  import Icon from "./Icon.svelte";
  let { id } = $props();
  let item = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    const reqId = id; // capture: ignore this response if `id` changes before it resolves
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/calls/${encodeURIComponent(id)}`);
      if (reqId !== id) return; // a newer id superseded this request
      if (r.status === 404) { notFound = true; item = null; return; }
      if (!r.ok) throw new Error(`/api/calls/${id} -> ${r.status}`);
      const next = await r.json();
      if (reqId !== id) return;
      item = next; error = null; notFound = false;
    } catch (e) { if (reqId === id) error = String(e); }
  }
  $effect(() => {
    id; // re-run load() whenever the id prop changes
    load();
  });
</script>

<a class="back" href="#/calls"><Icon name="back" size={14} /> Calls</a>
<h2>Call detail</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <div class="empty"><span class="ico"><Icon name="calls" size={28} /></span>
    <div>Call not found</div><div class="hint">it may have aged out of the live ring</div></div>
{:else if item}
  <div class="table-wrap"><table class="kv">
    <tbody>
      <tr><th>id</th><td>{item.id}</td></tr>
      <tr><th>time</th><td>{when(item.ts_unix_ms)}</td></tr>
      <tr><th>meta_tool</th><td>{item.meta_tool}</td></tr>
      <tr><th>target_tool</th><td>{#if item.target_tool}<a href={`#/tools/${encodeURIComponent(item.target_tool)}`}>{item.target_tool}</a>{:else}—{/if}</td></tr>
      <tr><th>upstream</th><td>{#if item.upstream}<a href={`#/upstreams/${encodeURIComponent(item.upstream)}`}>{item.upstream}</a>{:else}—{/if}</td></tr>
      <tr><th>outcome</th><td><span class="badge {item.outcome}">{item.outcome}</span></td></tr>
      <tr><th>error_kind</th><td>{item.error_kind ?? "—"}</td></tr>
      <tr><th>latency_ms</th><td>{item.latency_ms}</td></tr>
      <tr><th>arg_bytes</th><td>{item.arg_bytes}</td></tr>
      <tr><th>result_bytes</th><td>{item.result_bytes}</td></tr>
    </tbody>
  </table></div>

  <h3>Arguments{#if item.args_truncated} <span class="muted">(truncated)</span>{/if}</h3>
  {#if item.args != null}
    <pre class="schema">{pretty(item.args)}</pre>
  {:else}
    <p class="muted">(content not retained)</p>
  {/if}

  <h3>Result{#if item.result_truncated} <span class="muted">(truncated)</span>{/if}</h3>
  {#if item.result != null}
    <pre class="schema">{pretty(item.result)}</pre>
  {:else}
    <p class="muted">(content not retained)</p>
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(5) as _}<div class="sk row"></div>{/each}</div>
{/if}
