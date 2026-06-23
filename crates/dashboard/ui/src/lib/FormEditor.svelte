<script>
  import { SECTIONS, sectionReload } from "./configSchema.js";
  import SectionRetrieval from "./SectionRetrieval.svelte";
  import SectionServer from "./SectionServer.svelte";
  import SectionAudit from "./SectionAudit.svelte";
  import SectionDashboard from "./SectionDashboard.svelte";
  import SectionUpstreams from "./SectionUpstreams.svelte";

  let { model = $bindable() } = $props();
  let current = $state("retrieval");
  const LABELS = { retrieval: "Retrieval", server: "Server", audit: "Audit", dashboard: "Dashboard", upstream: "Upstreams" };
</script>

<div class="cfg-form">
  <nav class="cfg-nav">
    {#each SECTIONS as s}
      <button type="button" class="cfg-navitem" class:active={current === s} onclick={() => (current = s)}>
        <span>{LABELS[s]}</span>
        <span class="badge {sectionReload(s) === 'hot' ? 'ok' : 'skipped'}">{sectionReload(s) === 'hot' ? '🔥' : '⟳'}</span>
      </button>
    {/each}
  </nav>
  <div class="cfg-pane">
    {#if current === "retrieval"}<SectionRetrieval bind:retrieval={model.retrieval} />
    {:else if current === "server"}<SectionServer bind:server={model.server} />
    {:else if current === "audit"}<SectionAudit bind:audit={model.audit} />
    {:else if current === "dashboard"}<SectionDashboard bind:dashboard={model.dashboard} />
    {:else if current === "upstream"}<SectionUpstreams bind:upstream={model.upstream} />
    {/if}
  </div>
</div>
