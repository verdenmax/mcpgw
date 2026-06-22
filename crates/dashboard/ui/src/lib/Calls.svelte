<script>
  import { getJSON } from "./api.js";
  import { refresh } from "./refresh.svelte.js";
  import { go, when, ago } from "./format.js";
  import Icon from "./Icon.svelte";
  import Activity from "./Activity.svelte";
  import { pendingBucket } from "./bucketSel.svelte.js";

  const LIMIT = 50;
  let metrics = $state([]);     // per_meta_tool summary
  let source = $state("live");  // live | history
  let meta = $state("");        // "" = all meta-tools
  let outcome = $state("");     // "" = all outcomes
  let qtext = $state("");       // free-text content search
  let argKey = $state("");      // structured arg filter key
  let argVal = $state("");      // structured arg filter value
  let offset = $state(0);
  let resp = $state(null);      // CallsResponse
  let error = $state(null);
  let rangeMs = $state(900000); // 时间范围(ms)；0 = all。默认 15min
  let bucketSel = $state(null); // {since, until} 绝对窗（点柱所选）；与 rangeMs 互斥
  // 从 Overview 点柱跳转过来时，组件初始化消费一次暂存窗。
  if (pendingBucket.since != null) {
    bucketSel = { since: pendingBucket.since, until: pendingBucket.until };
    pendingBucket.since = null;
    pendingBucket.until = null;
  }

  const query = $derived.by(() => {
    const q = new URLSearchParams();
    q.set("source", source);
    if (meta) q.set("meta", meta);
    if (outcome) q.set("outcome", outcome);
    if (qtext) q.set("q", qtext);
    if (argKey && argVal) { q.set("arg_key", argKey); q.set("arg_val", argVal); }
    const bs = bucketSel; // 无条件读 -> query 始终把 bucketSel 当依赖
    if (bs) { q.set("since", String(bs.since)); q.set("until", String(bs.until)); }
    q.set("limit", String(LIMIT));
    q.set("offset", String(offset));
    return q.toString();
  });

  const anyFilter = $derived(!!(meta || outcome || qtext || (argKey && argVal)));

  async function loadMetrics() {
    // Metric cards are a secondary summary; on a /api/metrics blip we keep the last cards rather
    // than surfacing an error (the calls list below owns the error UI).
    try { const m = await getJSON("/api/metrics"); metrics = m.per_meta_tool ?? []; } catch (_) {}
  }
  async function loadCalls() {
    const reqQ = query; // discard a superseded response if the query changed mid-flight
    const reqRange = rangeMs; // `since` lives outside `query`, so guard the window separately
    // `since` is computed at request time (not in the memoized `query` derived) so the time window
    // slides with each refresh tick instead of freezing at the value from the last filter change.
    const since = !bucketSel && rangeMs > 0 ? `&since=${Date.now() - rangeMs}` : "";
    try {
      const r = await getJSON(`/api/calls?${query}${since}`);
      if (reqQ !== query || reqRange !== rangeMs) return;
      resp = r; error = null;
    } catch (e) { if (reqQ === query && reqRange === rangeMs) error = String(e); }
  }
  function pickMeta(m) { meta = meta === m ? "" : m; offset = 0; }
  function setSource(s) { source = s; offset = 0; }
  function setOutcome(o) { outcome = outcome === o ? "" : o; offset = 0; }
  function pct(a, b) { return b > 0 ? Math.min(100, Math.round((a / b) * 100)) : 0; }
  function clearFilters() { meta = ""; outcome = ""; qtext = ""; argKey = ""; argVal = ""; offset = 0; }
  function setRange(ms) { rangeMs = ms; bucketSel = null; offset = 0; }

  // Refetch the list on any filter change (reading `query` tracks all of them) and each refresh tick.
  $effect(() => { void query; void rangeMs; void bucketSel; refresh.tick; loadCalls(); });
  $effect(() => { refresh.tick; loadMetrics(); });
</script>

<h2>Calls</h2>

<div class="cards">
  {#each metrics as m}
    <button class="card" class:active={meta === m.meta_tool} onclick={() => pickMeta(m.meta_tool)}>
      <div class="ctop"><span class="label">{m.meta_tool}</span><span class="ico-badge"><Icon name="bolt" /></span></div>
      <div class="v num">{m.calls}</div>
      <div class="sub" class:bad={m.errors > 0}>{m.errors} error{m.errors === 1 ? "" : "s"}</div>
      {#if m.max_ms > 0}
        <div class="bars">
          <div class="bar"><span>p50</span><span class="track"><span class="fill" style="width:{pct(m.p50_ms, m.max_ms)}%"></span></span><span class="num">{m.p50_ms}ms</span></div>
          <div class="bar"><span>p95</span><span class="track"><span class="fill warn" style="width:{pct(m.p95_ms, m.max_ms)}%"></span></span><span class="num">{m.p95_ms}ms</span></div>
        </div>
        <div class="sub">max {m.max_ms}ms</div>
      {:else}
        <div class="sub subtle">no latency yet</div>
      {/if}
    </button>
  {/each}
</div>

<div class="chips">
  {#each [["5m", 300000], ["15m", 900000], ["1h", 3600000], ["24h", 86400000], ["all", 0]] as [lbl, ms]}
    <button class="chip" class:active={!bucketSel && rangeMs === ms} onclick={() => setRange(ms)}>{lbl}</button>
  {/each}
  {#if bucketSel}<button class="chip active" onclick={() => { bucketSel = null; offset = 0; }}>bucket: {new Date(bucketSel.since).toLocaleTimeString()}–{new Date(bucketSel.until).toLocaleTimeString()} ✕</button>{/if}
</div>
<Activity window={rangeMs > 0 ? rangeMs : 3600000} sections="spark,breakdown"
          onpick={(since, until) => { bucketSel = { since, until }; offset = 0; }} />

<div class="chips">
  <button class="chip" class:active={source === "live"} onclick={() => setSource("live")}>live</button>
  <button class="chip" class:active={source === "history"} onclick={() => setSource("history")}>history</button>
  <span class="chip-sep">·</span>
  {#each ["ok", "error", "timeout"] as o}
    <button class="chip" class:active={outcome === o} onclick={() => setOutcome(o)}>{o}</button>
  {/each}
  {#if meta}<button class="chip active" onclick={() => pickMeta(meta)}>meta: {meta} ✕</button>{/if}
  {#if anyFilter}<button class="chip" onclick={clearFilters}>clear filters ✕</button>{/if}
</div>

<div class="toolbar">
  <input class="search" placeholder="search content (args/result)…" bind:value={qtext}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  <input class="search narrow" placeholder="arg key" bind:value={argKey}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  <input class="search narrow" placeholder="value" bind:value={argVal}
         oninput={() => (offset = 0)} disabled={source === "history"} />
  {#if source === "history"}<span class="muted">content filters apply to live only</span>{/if}
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if resp}
  {#if resp.source === "history" && resp.history_unavailable}
    <div class="empty"><span class="ico"><Icon name="calls" size={28} /></span>
      <div>History unavailable</div><div class="hint">enable <code>[audit]</code> to replay past calls</div></div>
  {:else if resp.items.length === 0}
    <div class="empty"><span class="ico"><Icon name="calls" size={28} /></span>
      {#if anyFilter || rangeMs > 0 || bucketSel}<div>No calls match these filters</div><div class="hint">adjust or clear the filters above</div>
      {:else}<div>No calls yet</div><div class="hint">invoke a meta-tool to see it here</div>{/if}</div>
  {:else}
    <p class="meta-line"><span class="count-pill">{resp.total}</span> total</p>
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>time</th><th>meta</th><th>target</th><th>upstream</th><th>outcome</th><th>error</th><th class="num">ms</th></tr></thead>
      <tbody>
        {#each resp.items as c}
          {@const href = `#/calls/${c.id}`}
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <tr class="row-link" onclick={() => go(href)}>
            <td class="num"><a class="rl" href={href} title={when(c.ts_unix_ms)}>{ago(c.ts_unix_ms)}</a></td>
            <td>{c.meta_tool}</td>
            <td class="mono">{c.target_tool ?? "—"}</td>
            <td class="mono">{c.upstream ?? "—"}</td>
            <td><span class="badge {c.outcome}">{c.outcome}</span></td>
            <td><span class:bad={c.error_kind}>{c.error_kind ?? "—"}</span></td>
            <td class="num">{c.latency_ms}</td>
          </tr>
        {/each}
      </tbody>
    </table></div></div>
    {#if resp.total > 0}
      <div class="chips" style="margin-top:var(--s3)">
        <button class="chip" disabled={offset === 0} onclick={() => (offset = Math.max(0, offset - LIMIT))}>‹ prev</button>
        <span class="muted num">{Math.min(offset + 1, resp.total)}–{Math.min(offset + LIMIT, resp.total)} of {resp.total}</span>
        <button class="chip" disabled={offset + LIMIT >= resp.total} onclick={() => { if (offset + LIMIT < resp.total) offset += LIMIT; }}>next ›</button>
      </div>
    {/if}
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(6) as _}<div class="sk row"></div>{/each}</div>
{/if}
