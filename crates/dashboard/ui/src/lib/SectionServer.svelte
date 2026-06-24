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
  <label class="cfg-field cfg-switch">stdio <input type="checkbox" bind:checked={server.stdio} /><span class="cfg-hint">是否开启 stdio 传输（供本地 MCP 客户端经标准输入输出连接）</span></label>
  {#if !server.http}
    <button type="button" class="iconbtn" onclick={enableHttp}>+ 启用 [server.http]</button>
  {:else}
    <div class="cfg-sub" role="group" aria-label="http">
      <div class="cfg-sub-h">http</div>
      <label class="cfg-field cfg-switch">enabled <input type="checkbox" bind:checked={server.http.enabled} /><span class="cfg-hint">是否开启 HTTP（Streamable HTTP）传输</span></label>
      <label class="cfg-field">bind
        <input bind:value={server.http.bind} />
        <span class="cfg-hint">HTTP 监听地址，host:port，如 127.0.0.1:8970</span>
      </label>
      <label class="cfg-field"><span class="cfg-lbl">path <span class="cfg-q" title="MCP 端点路径，默认 /mcp" aria-label="MCP 端点路径，默认 /mcp">?</span></span><input bind:value={server.http.path} /></label>
      <div class="cfg-arr"><span class="label">api_key <span class="cfg-q" title="每条：name=key 标签（仅日志/观测，非密钥本身）、env=存放该 key 的环境变量名" aria-label="每条：name=key 标签（仅日志/观测，非密钥本身）、env=存放该 key 的环境变量名">?</span></span>
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
