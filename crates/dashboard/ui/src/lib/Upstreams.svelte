<script>
  import { getJSON } from "./api.js";
  import { refresh } from "./refresh.svelte.js";
  import { go } from "./format.js";
  import Icon from "./Icon.svelte";
  import DisableToggle from "./DisableToggle.svelte";
  let ups = $state(null);
  let dis = $state({ upstreams: [], tools: [] });
  let error = $state(null);
  let sortKey = $state("name");
  let sortDir = $state(1); // 1 asc, -1 desc
  const disUp = $derived(new Set(dis.upstreams));
  async function load() {
    try { ups = await getJSON("/api/upstreams"); dis = await getJSON("/api/disabled"); error = null; }
    catch (e) { error = String(e); }
  }
  $effect(() => { refresh.tick; load(); });
  function sortBy(k) { if (sortKey === k) sortDir = -sortDir; else { sortKey = k; sortDir = 1; } }
  const rows = $derived.by(() => {
    const list = [...(ups ?? [])];
    list.sort((a, b) => {
      const av = a[sortKey], bv = b[sortKey];
      const c = typeof av === "number" && typeof bv === "number"
        ? av - bv : String(av).localeCompare(String(bv));
      return c * sortDir;
    });
    return list;
  });
  const cols = [["name", "name", ""], ["transport", "transport", ""], ["status", "status", ""],
               ["tools", "tools", "num"], ["calls", "calls", "num"], ["errors", "errors", "num"]];
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
      <thead><tr>
        {#each cols as [key, label, cls]}
          <th class={cls}><button class="th-sort" onclick={() => sortBy(key)}>{label}<span class="arrow">{sortKey === key ? (sortDir > 0 ? "▲" : "▼") : ""}</span></button></th>
        {/each}
      </tr></thead>
      <tbody>
        {#each rows as u}
          {@const href = `#/upstreams/${encodeURIComponent(u.name)}`}
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <tr class="row-link" onclick={() => go(href)}>
            <td class="mono"><a class="rl" href={href}>{u.name}</a></td>
            <td>{u.transport}</td>
            <td>
              <span class="badge {u.status}">{u.status}</span>
              {#if disUp.has(u.name)}<span class="badge skipped">disabled</span>{/if}
              {#if u.reason} <span class="muted">{u.reason}</span>{/if}
              <DisableToggle kind="upstreams" name={u.name} disabled={disUp.has(u.name)} />
            </td>
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
