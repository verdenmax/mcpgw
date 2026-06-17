<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import Nav from "./lib/Nav.svelte";
  import Overview from "./lib/Overview.svelte";
  import Calls from "./lib/Calls.svelte";
  import CallDetail from "./lib/CallDetail.svelte";
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
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
