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
    const h = {}; for (const [k, v] of Object.entries(u.headers)) h[k === oldK ? newK : k] = v; u.headers = h;
  }
  function rmHeader(u, k) { const h = { ...u.headers }; delete h[k]; u.headers = h; }
</script>

{#if !upstream || upstream.length === 0}
  <p class="muted">无 upstream。</p>
{/if}
{#each upstream ?? [] as u, i}
  <fieldset class="cfg-sub cfg-upstream">
    <legend>upstream[{i}] <button type="button" class="admbtn" onclick={() => remove(i)}>✕ 移除</button></legend>
    <label class="cfg-field">name <input bind:value={u.name} placeholder="唯一、非空、不含 __" /></label>
    <label class="cfg-field">call_timeout_ms <input type="number" min="1" bind:value={u.call_timeout_ms} /></label>
    <label class="cfg-field">transport
      <select bind:value={u.transport} onchange={() => onTransport(u)}>
        {#each TRANSPORTS as t}<option value={t}>{t}</option>{/each}
      </select>
    </label>
    {#if u.transport === "stdio"}
      <label class="cfg-field">command <input bind:value={u.command} placeholder="可执行路径" /></label>
      <label class="cfg-field">args <input value={(u.args ?? []).join(" ")} oninput={(e) => (u.args = e.target.value.split(/\s+/).filter(Boolean))} placeholder="空格分隔" /></label>
      <label class="cfg-field">env_passthrough <input value={(u.env_passthrough ?? []).join(" ")} oninput={(e) => (u.env_passthrough = e.target.value.split(/\s+/).filter(Boolean))} placeholder="如 PATH HOME" /></label>
    {:else if u.transport === "http"}
      <label class="cfg-field">url <input bind:value={u.url} placeholder="https://…/mcp" /></label>
      <label class="cfg-field">bearer_env <input bind:value={u.bearer_env} placeholder="环境变量名(可选)" /></label>
      <div class="cfg-arr"><span class="label">headers (header名 → env名)</span>
        {#each Object.entries(u.headers ?? {}) as [k, v]}
          <div class="cfg-arr-row">
            <input value={k} onchange={(e) => setHeaderKey(u, k, e.target.value)} placeholder="header 名" />
            <input value={v} onchange={(e) => (u.headers[k] = e.target.value)} placeholder="env 变量名" />
            <button type="button" class="admbtn" onclick={() => rmHeader(u, k)}>✕</button>
          </div>
        {/each}
        <button type="button" class="admbtn" onclick={() => addHeader(u)}>+ add header</button>
      </div>
    {/if}
  </fieldset>
{/each}
<button type="button" class="admbtn" onclick={add}>+ add upstream</button>
