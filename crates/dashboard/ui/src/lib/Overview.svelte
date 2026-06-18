<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  let data = $state(null);
  let error = $state(null);
  async function load() {
    try { data = await getJSON("/api/overview"); error = null; }
    catch (e) { error = String(e); }
  }
  onMount(() => {
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  });
</script>

<h2>Overview</h2>
{#if error}<p class="error">{error}</p>{/if}
{#if data}
  <div class="cards">
    <div class="card row-link" onclick={() => (location.hash = "#/upstreams")}><div class="label">upstreams</div><div class="v">{data.upstreams_connected}/{data.upstreams_total}</div></div>
    <div class="card row-link" onclick={() => (location.hash = "#/tools")}><div class="label">tools</div><div class="v">{data.tools_total}</div></div>
    <div class="card row-link" onclick={() => (location.hash = "#/calls")}><div class="label">calls</div><div class="v">{data.total_calls}</div></div>
    <div class="card"><div class="label">strategy</div><div class="v">{data.strategy}</div></div>
    <div class="card"><div class="label">uptime</div><div class="v">{data.uptime_secs}s</div></div>
    <div class="card"><div class="label">rebuild skipped</div><div class="v">{data.last_rebuild_skipped}</div></div>
  </div>
{:else if !error}
  <p class="muted">loading…</p>
{/if}
