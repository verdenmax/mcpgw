<script>
  import { admin, adminGet, adminPut } from "./admin.svelte.js";
  import RawEditor from "./RawEditor.svelte";
  let content = $state("");
  let loaded = $state(false);
  let error = $state(null);
  let result = $state(null);
  let busy = $state(false);
  let reqId = 0;
  let view = $state("raw"); // "raw" | "form"

  async function load() {
    busy = true; error = null; result = null;
    const my = ++reqId;
    try {
      const r = await adminGet("/api/admin/config");
      if (my !== reqId) return; // superseded by a newer load
      if (r.status === 404) { error = "serve 未带 --config（无文件可改）"; loaded = false; return; }
      if (r.status === 401) { error = "admin token 失效，请在 About 重新输入"; loaded = false; return; }
      if (!r.ok) { error = `GET ${r.status}: ${await r.text()}`; loaded = false; return; }
      content = (await r.json()).content; loaded = true;
    } catch (e) { if (my === reqId) error = String(e); }
    finally { if (my === reqId) busy = false; }
  }

  async function save() {
    busy = true; error = null; result = null;
    try {
      const r = await adminPut("/api/admin/config", content);
      if (r.status === 200) result = await r.json();
      else error = `${r.status}: ${await r.text()}`;
    } catch (e) { error = String(e); }
    finally { busy = false; }
  }

  $effect(() => { if (admin.token && !loaded) load(); });
</script>

<h2>Config</h2>
{#if !admin.token}
  <p class="muted">需要 admin token（在 About 页输入）才能编辑配置。</p>
{:else}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if loaded}
    <div class="cfg-modes">
      <button class="admbtn" class:active={view === "raw"} onclick={() => (view = "raw")}>Raw</button>
      <button class="admbtn" class:active={view === "form"} onclick={() => (view = "form")}>Form</button>
    </div>
    {#if view === "raw"}
      <RawEditor bind:content />
    {:else}
      <p class="muted">表单模式将在 Task 5–7 接入。</p>
    {/if}
    <div class="toolbar">
      <button class="admbtn" onclick={save} disabled={busy}>{busy ? "saving…" : "Save"}</button>
      <button class="admbtn" onclick={load} disabled={busy}>Reload</button>
    </div>
    {#if result}
      <div class="card" style="margin-top:var(--s3)">
        <p>✓ saved · upstreams +{result.upstreams.added.length} −{result.upstreams.removed.length} ~{result.upstreams.reconnected.length}
          {#if result.upstreams.connect_failures.length}
            <span class="badge error" title={result.upstreams.connect_failures.map((f) => f[1]).join("; ")}>connect failed: {result.upstreams.connect_failures.map((f) => f[0]).join(", ")}</span>
          {/if}
        </p>
        {#if result.needs_restart.length}
          <p><span class="badge skipped">需重启生效</span> {result.needs_restart.join(", ")}</p>
        {/if}
      </div>
    {/if}
  {/if}
{/if}
