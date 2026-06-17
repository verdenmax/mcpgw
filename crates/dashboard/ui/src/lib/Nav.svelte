<script>
  import { route } from "./router.svelte.js";
  const items = [
    ["overview", "Overview"],
    ["upstreams", "Upstreams"],
    ["tools", "Tools"],
    ["calls", "Calls"],
    ["traces", "Traces"],
  ];
</script>

<nav class="sidebar">
  <div class="brand">mcpgw</div>
  <ul>
    {#each items as [view, label]}
      <li class:active={route.view === view}><a href={`#/${view}`}>{label}</a></li>
    {/each}
  </ul>
</nav>
