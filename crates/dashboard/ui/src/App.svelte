<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import Nav from "./lib/Nav.svelte";
  import Overview from "./lib/Overview.svelte";
  import Calls from "./lib/Calls.svelte";
  import CallDetail from "./lib/CallDetail.svelte";
  import Upstreams from "./lib/Upstreams.svelte";
  import Tools from "./lib/Tools.svelte";
  import Traces from "./lib/Traces.svelte";
  onMount(startRouter);
</script>

<div class="layout">
  <Nav />
  <main class="content">
    {#if route.view === "overview"}
      <Overview />
    {:else if route.view === "calls" && route.params.length > 0}
      <CallDetail id={route.params[0]} />
    {:else if route.view === "calls"}
      <Calls />
    {:else if route.view === "upstreams"}
      <Upstreams />
    {:else if route.view === "tools"}
      <Tools />
    {:else if route.view === "traces"}
      <Traces />
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
