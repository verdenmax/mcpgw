<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  import { go } from "./format.js";
  import Icon from "./Icon.svelte";
  let ups = $state(null);
  let error = $state(null);
  async function load() {
    try { ups = await getJSON("/api/upstreams"); error = null; }
    catch (e) { error = String(e); }
  }
  onMount(() => { load(); const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Upstreams</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if ups}
  <p class="meta-line"><span class="count-pill">{ups.length}</span> upstream{ups.length === 1 ? "" : "s"}</p>
  {#if ups.length === 0}
    <div class="empty"><span class="ico"><Icon name="server" size={28} /></span>
      <div>No upstreams configured</div>
      <div class="hint">add an <code>[[upstream]]</code> section to mcpgw.toml</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>name</th><th>transport</th><th>status</th><th class="num">tools</th><th class="num">calls</th><th class="num">errors</th></tr></thead>
      <tbody>
        {#each ups as u}
          {@const href = `#/upstreams/${encodeURIComponent(u.name)}`}
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <tr class="row-link" onclick={() => go(href)}>
            <td class="mono"><a class="rl" href={href}>{u.name}</a></td>
            <td>{u.transport}</td>
            <td><span class="badge {u.status}">{u.status}</span>{#if u.reason} <span class="muted">{u.reason}</span>{/if}</td>
            <td class="num">{u.tools}</td>
            <td class="num">{u.calls}</td>
            <td class="num" class:bad={u.errors > 0}>{u.errors}</td>
          </tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(4) as _}<div class="sk row"></div>{/each}</div>
{/if}
