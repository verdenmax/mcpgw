<script>
  import { route } from "./router.svelte.js";
  import { admin } from "./admin.svelte.js";
  import Icon from "./Icon.svelte";
  const base = [
    ["overview", "Overview", "overview"],
    ["upstreams", "Upstreams", "upstreams"],
    ["tools", "Tools", "tools"],
    ["calls", "Calls", "calls"],
    ["traces", "Traces", "traces"],
    ["about", "About", "info"],
  ];
  const items = $derived(admin.token ? [...base, ["config", "Config", "wrench"]] : base);
</script>

<nav class="sidebar">
  <div class="group">Navigate</div>
  <ul>
    {#each items as [view, label, icon]}
      <li class:active={route.view === view}>
        <a href={`#/${view}`} aria-current={route.view === view ? "page" : undefined}>
          <span class="ico"><Icon name={icon} /></span>{label}
        </a>
      </li>
    {/each}
  </ul>
</nav>
