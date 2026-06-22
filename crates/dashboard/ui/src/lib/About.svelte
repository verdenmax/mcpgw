<script>
  import { onMount } from "svelte";
  import { getJSON } from "./api.js";
  import { admin } from "./admin.svelte.js";
  let info = $state(null);
  let error = $state(null);
  onMount(async () => {
    try { info = await getJSON("/api/about"); }
    catch (e) { error = String(e); }
  });
  function built(secs) {
    const n = Number(secs);
    return n > 0 ? new Date(n * 1000).toLocaleString() : "unknown";
  }
</script>

<h2>About</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if info}
  <h3>Version</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>version</th><td>{info.version.version}</td></tr>
    <tr><th>git</th><td class="mono">{info.version.git_sha}</td></tr>
    <tr><th>built</th><td>{built(info.version.build_time)}</td></tr>
  </tbody></table></div>

  <h3>Retrieval</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>strategy</th><td>{info.retrieval.strategy}</td></tr>
    <tr><th>top_k</th><td>{info.retrieval.top_k}</td></tr>
  </tbody></table></div>

  <h3>Dashboard</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>call_buffer</th><td>{info.dashboard.call_buffer}</td></tr>
    <tr><th>payload_max_bytes</th><td>{info.dashboard.payload_max_bytes}</td></tr>
    <tr><th>trace_queries</th><td>{info.dashboard.trace_queries}</td></tr>
    <tr><th>trace_buffer</th><td>{info.dashboard.trace_buffer}</td></tr>
    <tr><th>trace_path</th><td class="mono">{info.dashboard.trace_path ?? "—"}</td></tr>
  </tbody></table></div>

  <h3>Audit</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>enabled</th><td><span class="badge {info.audit.enabled ? 'ok' : 'unknown'}">{info.audit.enabled ? "on" : "off"}</span></td></tr>
    <tr><th>path</th><td class="mono">{info.audit.path ?? "—"}</td></tr>
  </tbody></table></div>

  <h3>Server</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>stdio</th><td>{info.server.stdio}</td></tr>
    <tr><th>http</th><td><span class="badge {info.server.http_enabled ? 'ok' : 'unknown'}">{info.server.http_enabled ? "enabled" : "disabled"}</span></td></tr>
    <tr><th>http_bind</th><td class="mono">{info.server.http_bind ?? "—"}</td></tr>
    <tr><th>http_path</th><td class="mono">{info.server.http_path ?? "—"}</td></tr>
    <tr><th>http_auth</th><td><span class="badge {info.server.http_auth ? 'ok' : 'unknown'}">{info.server.http_auth ? "enabled" : "disabled"}</span></td></tr>
  </tbody></table></div>

  <h3>Admin (write access)</h3>
  <div class="table-wrap"><table class="kv"><tbody>
    <tr><th>status</th><td><span class="badge {info.dashboard.admin_enabled ? 'ok' : 'unknown'}">{info.dashboard.admin_enabled ? "enabled" : "disabled"}</span></td></tr>
  </tbody></table></div>
  {#if info.dashboard.admin_enabled}
    <p class="hint">Paste the admin token to unlock disable/enable controls. Held in memory only (cleared on refresh).</p>
    <input class="search" type="password" placeholder="admin token…" autocomplete="off" aria-label="admin token" bind:value={admin.token} />
  {/if}

  <h3>Upstreams ({info.upstreams.length})</h3>
  {#if info.upstreams.length === 0}
    <div class="empty"><div>No upstreams configured</div></div>
  {:else}
    <div class="table-wrap"><div class="table-scroll"><table>
      <thead><tr><th>name</th><th>transport</th><th class="num">timeout_ms</th></tr></thead>
      <tbody>
        {#each info.upstreams as u}
          <tr><td class="mono">{u.name}</td><td>{u.transport}</td><td class="num">{u.call_timeout_ms}</td></tr>
        {/each}
      </tbody>
    </table></div></div>
  {/if}
{:else if !error}
  <div class="skeleton">{#each Array(6) as _}<div class="sk row"></div>{/each}</div>
{/if}
