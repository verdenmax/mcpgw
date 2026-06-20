<script>
  import { getJSON } from "./api.js";
  import { refresh } from "./refresh.svelte.js";
  import { go } from "./format.js";
  import Icon from "./Icon.svelte";
  let q = $state("");
  let tools = $state(null);
  let error = $state(null);
  async function load() {
    const reqQ = q; // discard a superseded response if the filter changed mid-flight
    try {
      const qs = q ? `?q=${encodeURIComponent(q)}` : "";
      const t = await getJSON(`/api/tools${qs}`);
      if (reqQ !== q) return;
      tools = t; error = null;
    } catch (e) { if (reqQ === q) error = String(e); }
  }
  // Refetch on every `q` change and on each global refresh tick.
  $effect(() => { void q; refresh.tick; load(); });
</script>

<h2>Tools</h2>
<div class="toolbar">
  <input class="search" placeholder="filter tools…" bind:value={q} />
  {#if tools}<span class="meta-line" style="margin:0"><span class="count-pill">{tools.length}</span> match{tools.length === 1 ? "" : "es"}</span>{/if}
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if tools}
  {#if tools.length === 0}
    <div class="empty"><span class="ico"><Icon name="tools" size={28} /></span>
      <div>{q ? "No tools match this filter" : "No tools available"}</div>
      <div class="hint">{q ? "try a different query" : "connect an upstream that exposes tools"}</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>name</th><th>description</th></tr></thead>
      <tbody>
        {#each tools as t}
          {@const href = `#/tools/${encodeURIComponent(t.name)}`}
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <tr class="row-link" onclick={() => go(href)}>
            <td class="mono"><a class="rl" href={href}>{t.name}</a></td><td>{t.description}</td>
          </tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(5) as _}<div class="sk row"></div>{/each}</div>
{/if}
