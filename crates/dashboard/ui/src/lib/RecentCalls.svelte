<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  import { go, rowKey, when } from "./format.js";
  import Icon from "./Icon.svelte";

  // Shared "Recent calls" panel for the Upstream/Tool detail pages. Pinned to source=live and to
  // one upstream/tool; owns its own outcome + content filters and its own 3s poll (so the parent's
  // detail load() no longer needs to fetch calls — avoiding a duplicate request).
  let { param, name } = $props(); // param: "upstream" | "tool"
  let calls = $state([]);
  let cOutcome = $state(""); // recent-calls outcome filter
  let cq = $state("");       // recent-calls content search

  // Composite request token: a response is applied only if name + both filters still match, so a
  // slow earlier fetch can't overwrite the list after the user changed the name/outcome/search.
  function token() { return `${name}\u0000${cOutcome}\u0000${cq}`; }
  async function loadCalls() {
    const req = token();
    try {
      const p = new URLSearchParams();
      p.set("source", "live");
      p.set(param, name);
      if (cOutcome) p.set("outcome", cOutcome);
      if (cq) p.set("q", cq);
      p.set("limit", "20");
      const c = await getJSON(`/api/calls?${p}`);
      if (req === token()) calls = c.items ?? [];
    } catch (_) { /* recent-calls is secondary; the detail page owns the error UI */ }
  }
  $effect(() => { void name; void cOutcome; void cq; loadCalls(); });
  onMount(() => { const t = setInterval(loadCalls, 3000); return () => clearInterval(t); });
</script>

<h3>Recent calls</h3>
<div class="chips">
  {#each ["ok", "error", "timeout"] as o}
    <button class="chip" class:active={cOutcome === o} onclick={() => (cOutcome = cOutcome === o ? "" : o)}>{o}</button>
  {/each}
  <input class="search narrow" placeholder="search content…" bind:value={cq} />
</div>
{#if calls.length === 0}
  <div class="empty"><span class="ico"><Icon name="calls" size={24} /></span><div>No recent calls</div></div>
{:else}
  <div class="table-wrap"><div class="table-scroll"><table>
    <thead><tr><th>time</th><th>meta</th>{#if param === "upstream"}<th>target</th>{/if}<th>outcome</th><th class="num">ms</th></tr></thead>
    <tbody>
      {#each calls as c}
        {@const href = `#/calls/${c.id}`}
        <tr class="row-link" role="button" tabindex="0" onclick={() => go(href)} onkeydown={rowKey(href)}>
          <td class="num">{when(c.ts_unix_ms)}</td>
          <td>{c.meta_tool}</td>
          {#if param === "upstream"}<td class="mono">{c.target_tool ?? "—"}</td>{/if}
          <td><span class="badge {c.outcome}">{c.outcome}</span></td>
          <td class="num">{c.latency_ms}</td>
        </tr>
      {/each}
    </tbody>
  </table></div></div>
{/if}
