<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import { refresh, startRefresh, refreshNow, togglePause } from "./lib/refresh.svelte.js";
  import { ago } from "./lib/format.js";
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
  import About from "./lib/About.svelte";

  let now = $state(Date.now()); // 1s clock so the "updated Ns ago" label ticks
  const updatedAgo = $derived.by(() => { void now; return ago(refresh.at); });
  onMount(() => {
    const stopRouter = startRouter();
    const stopRefresh = startRefresh();
    const clock = setInterval(() => (now = Date.now()), 1000);
    return () => { stopRouter?.(); stopRefresh(); clearInterval(clock); };
  });
</script>

<div class="layout">
  <h1 class="sr-only">mcpgw dashboard</h1>
  <div class="brandbar">
    <span class="logo"><Icon name="layers" size={17} /></span>
    <span class="name">mcpgw <span>· dashboard</span></span>
  </div>
  <header class="topbar">
    <span class="updated">updated {updatedAgo}</span>
    <button class="iconbtn" onclick={refreshNow} title="refresh now" aria-label="refresh now"><Icon name="refresh" size={15} /></button>
    <button class="live" class:paused={refresh.paused} onclick={togglePause}
            aria-pressed={refresh.paused} title={refresh.paused ? "resume auto-refresh" : "pause auto-refresh"}>
      <span class="dot"></span> {refresh.paused ? "paused" : "live"}
    </button>
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
    {:else if route.view === "about"}
      <About />
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
