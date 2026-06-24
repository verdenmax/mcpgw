<script>
  import { defaultSection } from "./configSchema.js";
  let { server = $bindable() } = $props();
  function enableHttp() { server.http = { enabled: false, bind: "127.0.0.1:8970", path: "/mcp", api_key: [] }; }
  function addKey() { server.http.api_key = [...(server.http.api_key ?? []), { name: "", env: "" }]; }
  function rmKey(i) { server.http.api_key = server.http.api_key.filter((_, j) => j !== i); }
</script>

{#if server === undefined}
  <button type="button" class="iconbtn" onclick={() => (server = defaultSection("server"))}>+ 启用 [server]</button>
{:else}
  <label class="cfg-field cfg-switch">stdio <input type="checkbox" bind:checked={server.stdio} /></label>
  {#if !server.http}
    <button type="button" class="iconbtn" onclick={enableHttp}>+ 启用 [server.http]</button>
  {:else}
    <div class="cfg-sub" role="group" aria-label="http">
      <div class="cfg-sub-h">http</div>
      <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={server.http.enabled} /></label>
      <label class="cfg-field">bind <input bind:value={server.http.bind} /></label>
      <label class="cfg-field">path <input bind:value={server.http.path} /></label>
      <div class="cfg-arr"><span class="label">api_key</span>
        {#each server.http.api_key ?? [] as k, i}
          <div class="cfg-arr-row">
            <input placeholder="name(标签)" bind:value={k.name} />
            <input placeholder="env(变量名)" bind:value={k.env} />
            <button type="button" class="iconbtn" onclick={() => rmKey(i)}>✕</button>
          </div>
        {/each}
        <button type="button" class="iconbtn" onclick={addKey}>+ add api_key</button>
      </div>
    </div>
  {/if}
{/if}
