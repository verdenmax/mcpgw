<script>
  import { getJSON } from "./api.js";
  import { refresh } from "./refresh.svelte.js";
  import Icon from "./Icon.svelte";
  let data = $state(null);
  let error = $state(null);
  async function load() {
    try { data = await getJSON("/api/overview"); error = null; }
    catch (e) { error = String(e); }
  }
  $effect(() => { refresh.tick; load(); });
</script>

<h2>Overview</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if data}
  <div class="cards">
    <a class="card" href="#/upstreams">
      <div class="ctop"><span class="label">upstreams</span><span class="ico-badge"><Icon name="server" /></span></div>
      <div class="v num">{data.upstreams_connected}<span class="subtle">/{data.upstreams_total}</span></div>
      <div class="sub">connected of total</div>
    </a>
    <a class="card" href="#/tools">
      <div class="ctop"><span class="label">tools</span><span class="ico-badge"><Icon name="tools" /></span></div>
      <div class="v num">{data.tools_total}</div>
      <div class="sub">routable across upstreams</div>
    </a>
    <a class="card" href="#/calls">
      <div class="ctop"><span class="label">calls</span><span class="ico-badge"><Icon name="calls" /></span></div>
      <div class="v num">{data.total_calls}</div>
      <div class="sub">meta-tool invocations</div>
    </a>
    <div class="card">
      <div class="ctop"><span class="label">strategy</span><span class="ico-badge"><Icon name="gauge" /></span></div>
      <div class="v sm">{data.strategy}</div>
      <div class="sub">retrieval backend</div>
    </div>
    <div class="card">
      <div class="ctop"><span class="label">uptime</span><span class="ico-badge"><Icon name="clock" /></span></div>
      <div class="v sm num">{data.uptime_secs}s</div>
      <div class="sub">since process start</div>
    </div>
    <div class="card">
      <div class="ctop"><span class="label">rebuild skipped</span><span class="ico-badge"><Icon name="layers" /></span></div>
      <div class="v sm">{data.last_rebuild_skipped}</div>
      <div class="sub">last snapshot rebuild</div>
    </div>
  </div>
{:else if !error}
  <div class="cards">
    {#each Array(6) as _}<div class="sk card"></div>{/each}
  </div>
{/if}
