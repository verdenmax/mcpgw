<script>
  import { onMount } from "svelte";
  import { route, startRouter } from "./lib/router.svelte.js";
  import Nav from "./lib/Nav.svelte";
  import Overview from "./lib/Overview.svelte";
  onMount(startRouter);
</script>

<div class="layout">
  <Nav />
  <main class="content">
    {#if route.view === "overview"}
      <Overview />
    {:else}
      <p class="muted">coming soon</p>
    {/if}
  </main>
</div>
