<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let ups = $state([]);
  let error = $state(null);
  async function load() {
    try { ups = await getJSON("/api/upstreams"); error = null; }
    catch (e) { error = String(e); }
  }
  onMount(() => { load(); const t = setInterval(load, 3000); return () => clearInterval(t); });
</script>

<h2>Upstreams</h2>
{#if error}<p class="error">{error}</p>{/if}
<table>
  <thead><tr><th>name</th><th>transport</th><th>status</th><th>tools</th><th>calls</th><th>errors</th></tr></thead>
  <tbody>
    {#each ups as u}
      <tr>
        <td>{u.name}</td>
        <td>{u.transport}</td>
        <td><span class="badge {u.status}">{u.status}</span>{#if u.reason} {u.reason}{/if}</td>
        <td>{u.tools}</td>
        <td>{u.calls}</td>
        <td>{u.errors}</td>
      </tr>
    {/each}
  </tbody>
</table>
