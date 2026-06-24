<script>
  import { TRANSPORTS } from "./configSchema.js";
  let { upstream = $bindable() } = $props();

  function add() {
    upstream = [...(upstream ?? []), { name: "", transport: "stdio", command: "", call_timeout_ms: 30000 }];
  }
  function remove(i) { upstream = upstream.filter((_, j) => j !== i); }

  // Drop the other transport's fields on switch so serialized TOML stays clean.
  function onTransport(u) {
    if (u.transport === "stdio") {
      delete u.url; delete u.bearer_env; delete u.headers;
      if (u.command === undefined) u.command = "";
    } else {
      delete u.command; delete u.args; delete u.env_passthrough;
      if (u.url === undefined) u.url = "";
    }
  }

  function addHeader(u) { u.headers = { ...(u.headers ?? {}), "": "" }; }
  function setHeaderKey(u, oldK, newK) {
    if (newK !== oldK && Object.prototype.hasOwnProperty.call(u.headers ?? {}, newK)) return; // refuse collision (would drop a value)
    const h = {}; for (const [k, v] of Object.entries(u.headers ?? {})) h[k === oldK ? newK : k] = v; u.headers = h;
  }
  function rmHeader(u, k) { const h = { ...u.headers }; delete h[k]; u.headers = h; }
</script>

{#if !upstream || upstream.length === 0}
  <p class="muted">无 upstream。</p>
{/if}
{#each upstream ?? [] as u, i}
  <div class="cfg-sub" role="group" aria-label={`upstream ${i}`}>
    <div class="cfg-sub-h">upstream[{i}] <button type="button" class="iconbtn" onclick={() => remove(i)}>✕ 移除</button></div>
    <label class="cfg-field">name
      <input bind:value={u.name} placeholder="唯一、非空、不含 __" />
      <span class="cfg-hint">该上游工具的命名空间前缀；非空、唯一、不含 __</span>
    </label>
    <label class="cfg-field">transport
      <select bind:value={u.transport} onchange={() => onTransport(u)}>
        {#each TRANSPORTS as t}<option value={t}>{t}</option>{/each}
      </select>
      <span class="cfg-hint">连接方式：stdio=本地子进程、http=远程 Streamable HTTP</span>
    </label>
    <label class="cfg-field"><span class="cfg-lbl">call_timeout_ms <span class="cfg-q" title="单次工具调用超时（毫秒，默认 30000）" aria-label="单次工具调用超时（毫秒，默认 30000）">?</span></span><input type="number" min="1" bind:value={u.call_timeout_ms} /></label>
    {#if u.transport === "stdio"}
      <label class="cfg-field">command
        <input bind:value={u.command} placeholder="可执行路径" />
        <span class="cfg-hint">子进程可执行文件路径</span>
      </label>
      <label class="cfg-field"><span class="cfg-lbl">args <span class="cfg-q" title="子进程启动参数（空格分隔；含空格的参数请用 raw 模式）" aria-label="子进程启动参数（空格分隔；含空格的参数请用 raw 模式）">?</span></span><input value={(u.args ?? []).join(" ")} oninput={(e) => (u.args = e.target.value.split(/\s+/).filter(Boolean))} placeholder="空格分隔" /></label>
      <label class="cfg-field"><span class="cfg-lbl">env_passthrough <span class="cfg-q" title="透传给子进程的环境变量名（其余环境被清空）" aria-label="透传给子进程的环境变量名（其余环境被清空）">?</span></span><input value={(u.env_passthrough ?? []).join(" ")} oninput={(e) => (u.env_passthrough = e.target.value.split(/\s+/).filter(Boolean))} placeholder="如 PATH HOME" /></label>
    {:else if u.transport === "http"}
      <label class="cfg-field">url
        <input bind:value={u.url} placeholder="https://…/mcp" />
        <span class="cfg-hint">远程 MCP 端点 URL，如 https://…/mcp</span>
      </label>
      <label class="cfg-field"><span class="cfg-lbl">bearer_env <span class="cfg-q" title="存放 Bearer token 的环境变量名（→ Authorization: Bearer，可选）" aria-label="存放 Bearer token 的环境变量名（→ Authorization: Bearer，可选）">?</span></span><input bind:value={u.bearer_env} placeholder="环境变量名(可选)" /></label>
      <div class="cfg-arr"><span class="label">headers <span class="cfg-q" title="自定义请求头：header 名 → 存放其值的环境变量名" aria-label="自定义请求头：header 名 → 存放其值的环境变量名">?</span></span>
        {#each Object.entries(u.headers ?? {}) as [k, v]}
          <div class="cfg-arr-row">
            <input value={k} onchange={(e) => setHeaderKey(u, k, e.target.value)} placeholder="header 名" />
            <input value={v} onchange={(e) => (u.headers[k] = e.target.value)} placeholder="env 变量名" />
            <button type="button" class="iconbtn" onclick={() => rmHeader(u, k)}>✕</button>
          </div>
        {/each}
        <button type="button" class="iconbtn" onclick={() => addHeader(u)}>+ add header</button>
      </div>
    {/if}
  </div>
{/each}
<button type="button" class="iconbtn" onclick={add}>+ add upstream</button>
