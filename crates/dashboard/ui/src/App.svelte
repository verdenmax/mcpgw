<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import Nav from "./lib/Nav.svelte";
  import Icon from "./lib/Icon.svelte";
  import Overview from "./lib/Overview.svelte";
  import Calls from "./lib/Calls.svelte";
  import CallDetail from "./lib/CallDetail.svelte";
  import Upstreams from "./lib/Upstreams.svelte";
  import UpstreamDetail from "./lib/UpstreamDetail.svelte";
  import Tools from "./lib/Tools.svelte";
  import ToolDetail from "./lib/ToolDetail.svelte";
  import Traces from "./lib/Traces.svelte";
  import TraceDetail from "./lib/TraceDetail.svelte";
  onMount(startRouter);
</script>

<div class="layout">
  <h1 class="sr-only">mcpgw dashboard</h1>
  <div class="brandbar">
    <span class="logo"><Icon name="layers" size={17} /></span>
    <span class="name">mcpgw <span>· dashboard</span></span>
  </div>
  <header class="topbar">
    <span class="live"><span class="dot"></span> live</span>
    <span class="refresh-hint">auto-refresh 3s</span>
  </header>
  <Nav />
  <main class="content">
    {#if route.view === "overview"}
      <Overview />
    {:else if route.view === "calls" && route.params.length > 0}
      <CallDetail id={route.params[0]} />
    {:else if route.view === "calls"}
      <Calls />
    {:else if route.view === "upstreams" && route.params.length > 0}
      <UpstreamDetail name={route.params[0]} />
    {:else if route.view === "upstreams"}
      <Upstreams />
    {:else if route.view === "tools" && route.params.length > 0}
      <ToolDetail name={route.params[0]} />
    {:else if route.view === "tools"}
      <Tools />
    {:else if route.view === "traces" && route.params.length > 0}
      <TraceDetail id={route.params[0]} />
    {:else if route.view === "traces"}
      <Traces />
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
