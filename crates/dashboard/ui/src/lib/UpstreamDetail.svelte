<script>
  import { onMount } from "svelte";
  import { go, rowKey } from "./format.js";
  import Icon from "./Icon.svelte";
  import RecentCalls from "./RecentCalls.svelte";
  let { name } = $props();
  let d = $state(null);
  let error = $state(null);
  let notFound = $state(false);
  async function load() {
    const reqName = name; // ignore a stale response if `name` changed before it resolved
    try {
      error = null; notFound = false;
      const r = await fetch(`/api/upstreams/${encodeURIComponent(name)}`);
      if (reqName !== name) return;
      if (r.status === 404) { notFound = true; d = null; return; }
      if (!r.ok) throw new Error(`/api/upstreams/${name} -> ${r.status}`);
      const next = await r.json();
      if (reqName === name) d = next;
    } catch (e) { if (reqName === name) error = String(e); }
  }
  $effect(() => { name; load(); });          // reload when the upstream name changes
  onMount(() => { const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<a class="back" href="#/upstreams"><Icon name="back" size={14} /> Upstreams</a>
{#if error}<p class="error">{error}</p>{/if}
{#if notFound}
  <div class="empty"><span class="ico"><Icon name="server" size={28} /></span><div>Upstream not found</div></div>
{:else if d}
  <h2>{d.name}</h2>
  <div class="cards">
    <div class="card"><div class="ctop"><span class="label">transport</span></div><div class="v sm">{d.transport}</div></div>
    <div class="card"><div class="ctop"><span class="label">status</span></div><div class="v sm"><span class="badge {d.status}">{d.status}</span></div></div>
    <div class="card"><div class="ctop"><span class="label">tools</span><span class="ico-badge"><Icon name="tools" /></span></div><div class="v num">{d.tools_count}</div></div>
    <div class="card"><div class="ctop"><span class="label">calls</span><span class="ico-badge"><Icon name="calls" /></span></div><div class="v num">{d.calls}</div></div>
    <div class="card"><div class="ctop"><span class="label">errors</span></div><div class="v num">{d.errors}</div></div>
  </div>
  {#if d.reason}<p class="meta-line">reason: {d.reason}</p>{/if}

  <h3>Tools</h3>
  {#if d.tools.length === 0}
    <div class="empty"><div>No tools exposed</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>name</th><th>description</th></tr></thead>
      <tbody>
        {#each d.tools as t}
          {@const href = `#/tools/${encodeURIComponent(t.name)}`}
          <tr class="row-link" role="button" tabindex="0" onclick={() => go(href)} onkeydown={rowKey(href)}>
            <td class="mono">{t.name}</td><td>{t.description}</td>
          </tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}

  <RecentCalls param="upstream" {name} />
{:else}
  <div class="skeleton">{#each Array(5) as _}<div class="sk row"></div>{/each}</div>
{/if}
