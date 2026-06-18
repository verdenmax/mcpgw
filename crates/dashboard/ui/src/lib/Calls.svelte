<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";

  const LIMIT = 50;
  let metrics = $state([]);     // per_meta_tool summary
  let source = $state("live");  // live | history
  let meta = $state("");        // "" = all meta-tools
  let outcome = $state("");     // "" = all outcomes
  let qtext = $state("");   // free-text content search
  let argKey = $state("");  // structured arg filter key
  let argVal = $state("");  // structured arg filter value
  let offset = $state(0);
  let resp = $state(null);      // CallsResponse
  let error = $state(null);

  const query = $derived.by(() => {
    const q = new URLSearchParams();
    q.set("source", source);
    if (meta) q.set("meta", meta);
    if (outcome) q.set("outcome", outcome);
    if (qtext) q.set("q", qtext);
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
    q.set("limit", String(LIMIT));
    q.set("offset", String(offset));
    return q.toString();
  });

  async function loadMetrics() {
    // Metric cards are a secondary summary; on a /api/metrics blip we keep the last cards rather
    // than surfacing an error (the calls list below owns the error UI).
    try { const m = await getJSON("/api/metrics"); metrics = m.per_meta_tool ?? []; } catch (_) {}
  }
  async function loadCalls() {
    try { resp = await getJSON(`/api/calls?${query}`); error = null; }
    catch (e) { error = String(e); }
  }
  function pickMeta(m) { meta = meta === m ? "" : m; offset = 0; }
  function setSource(s) { source = s; offset = 0; }
  function setOutcome(o) { outcome = outcome === o ? "" : o; offset = 0; }
  function when(ms) { return new Date(ms).toLocaleString(); }

  // Refetch the list whenever any filter changes (reading `query` tracks all of them).
  $effect(() => { void query; loadCalls(); });
  onMount(() => {
    loadMetrics();
    const t = setInterval(() => { loadMetrics(); loadCalls(); }, 3000);
    return () => clearInterval(t);
  });
</script>

<h2>Calls</h2>

<div class="cards">
  {#each metrics as m}
    <div class="card row-link" class:active={meta === m.meta_tool} onclick={() => pickMeta(m.meta_tool)}>
      <div class="label">{m.meta_tool}</div>
      <div class="v">{m.calls}</div>
      <div class="muted">err {m.errors} · p50 {m.p50_ms}ms · p95 {m.p95_ms}ms</div>
    </div>
  {/each}
</div>

<div class="chips">
  <span class="chip" class:active={source === "live"} onclick={() => setSource("live")}>live</span>
  <span class="chip" class:active={source === "history"} onclick={() => setSource("history")}>history</span>
  <span class="muted">·</span>
  {#each ["ok", "error", "timeout"] as o}
    <span class="chip" class:active={outcome === o} onclick={() => setOutcome(o)}>{o}</span>
  {/each}
  {#if meta}<span class="chip active" onclick={() => pickMeta(meta)}>meta: {meta} ✕</span>{/if}
</div>

<div class="chips">
  <input class="search" placeholder="search content (args/result)…" bind:value={qtext}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  <input class="search narrow" placeholder="arg key" bind:value={argKey}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  <input class="search narrow" placeholder="value" bind:value={argVal}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  {#if source === "history"}<span class="muted">content filters apply to live only</span>{/if}
</div>

{#if error}<p class="error">{error}</p>{/if}
{#if resp}
  {#if resp.source === "history" && resp.history_unavailable}
    <p class="muted">history unavailable (enable [audit])</p>
  {:else}
    <p class="muted">{resp.total} total</p>
    <table>
      <thead><tr><th>time</th><th>meta</th><th>target</th><th>upstream</th><th>outcome</th><th>ms</th></tr></thead>
      <tbody>
        {#each resp.items as c}
          <tr class="row-link" onclick={() => (location.hash = `#/calls/${c.id}`)}>
            <td>{when(c.ts_unix_ms)}</td>
            <td>{c.meta_tool}</td>
            <td>{c.target_tool ?? "—"}</td>
            <td>{c.upstream ?? "—"}</td>
            <td>{c.outcome}</td>
            <td>{c.latency_ms}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if resp.total > 0}
      <div class="chips">
        <span class="chip" class:disabled={offset === 0} onclick={() => (offset = Math.max(0, offset - LIMIT))}>‹ prev</span>
        <span class="muted">{Math.min(offset + 1, resp.total)}–{Math.min(offset + LIMIT, resp.total)}</span>
        <span class="chip" class:disabled={offset + LIMIT >= resp.total} onclick={() => { if (offset + LIMIT < resp.total) offset += LIMIT; }}>next ›</span>
      </div>
    {/if}
  {/if}
{:else if !error}
  <p class="muted">loading…</p>
{/if}
