<script>
  import { refresh } from "./refresh.svelte.js";
  import Icon from "./Icon.svelte";
  import RecentCalls from "./RecentCalls.svelte";
  import CopyButton from "./CopyButton.svelte";
  let { name } = $props();
  let d = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    const reqName = name; // ignore a stale response if `name` changed before it resolved
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/tools/${encodeURIComponent(name)}`);
      if (reqName !== name) return;
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/tools/${name} -> ${r.status}`);
      const next = await r.json();
      if (reqName === name) d = next;
    } catch (e) { if (reqName === name) error = String(e); }
  }
  $effect(() => { name; refresh.tick; load(); });
  function schema(v) { try { return JSON.stringify(v, null, 2); } catch (_) { return String(v); } }
</script>

<a class="back" href="#/tools"><Icon name="back" size={14} /> Tools</a>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notFound}
  <div class="empty"><span class="ico"><Icon name="tools" size={28} /></span><div>Tool not found</div></div>
{:else if d}
  <h2 class="mono">{d.name}</h2>
  <p class="meta-line">upstream: <a href={`#/upstreams/${encodeURIComponent(d.server)}`}>{d.server}</a></p>
  <p>{d.description}</p>
  <h3>Input schema</h3>
  {#if d.input_schema === null || d.input_schema === undefined}
    <p class="muted">(no schema)</p>
  {:else}
    <div class="codeblock"><CopyButton text={schema(d.input_schema)} /><pre class="schema">{schema(d.input_schema)}</pre></div>
  {/if}

  <RecentCalls param="tool" {name} />
{:else}
  <div class="skeleton">{#each Array(4) as _}<div class="sk row"></div>{/each}</div>
{/if}
