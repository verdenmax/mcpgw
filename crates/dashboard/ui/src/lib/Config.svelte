<script>
  import { admin, adminGet, adminPut } from "./admin.svelte.js";
  import { parseToml, stringifyToml } from "./toml.js";
  import { validateModel } from "./validate.js";
  import RawEditor from "./RawEditor.svelte";
  import FormEditor from "./FormEditor.svelte";

  let content = $state("");      // raw TOML text (source of truth for the raw view)
  let model = $state(null);      // parsed model (source of truth for the form view)
  let loaded = $state(false);
  let error = $state(null);
  let result = $state(null);
  let busy = $state(false);
  let view = $state("raw");      // "raw" | "form"
  let parseError = $state(null); // set when switching to form fails to parse
  let reqId = 0;

  const errors = $derived(view === "form" && model ? validateModel(model) : []);

  async function load() {
    busy = true; error = null; result = null; parseError = null;
    const my = ++reqId;
    try {
      const r = await adminGet("/api/admin/config");
      if (my !== reqId) return;
      if (r.status === 404) { error = "serve 未带 --config（无文件可改）"; loaded = false; return; }
      if (r.status === 401) { error = "admin token 失效，请在 About 重新输入"; loaded = false; return; }
      if (!r.ok) { error = `GET ${r.status}: ${await r.text()}`; loaded = false; return; }
      content = (await r.json()).content; loaded = true; view = "raw"; model = null;
    } catch (e) { if (my === reqId) error = String(e); }
    finally { if (my === reqId) busy = false; }
  }

  function toForm() {
    const r = parseToml(content);
    if (!r.ok) { parseError = r.error; view = "form"; model = null; return; }
    parseError = null; model = r.model; view = "form";
  }
  function toRaw() {
    if (model) content = stringifyToml(model);
    view = "raw";
  }

  async function save() {
    busy = true; error = null; result = null;
    try {
      const body = view === "form" && model ? stringifyToml(model) : content;
      content = body; // keep raw in sync with what we sent
      const r = await adminPut("/api/admin/config", body);
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
      <button class="admbtn" class:active={view === "raw"} aria-pressed={view === "raw"} onclick={toRaw}>Raw</button>
      <button class="admbtn" class:active={view === "form"} aria-pressed={view === "form"} onclick={toForm}>Form</button>
    </div>

    {#if view === "raw"}
      <RawEditor bind:content />
    {:else if parseError}
      <p class="error" role="alert">raw 有语法错误，修正后可结构化编辑：{parseError}</p>
    {:else if model}
      <FormEditor bind:model />
      {#if errors.length}
        <ul class="cfg-errs">{#each errors as e}<li><code>{e.path}</code> — {e.msg}</li>{/each}</ul>
      {/if}
    {/if}

    <div class="toolbar">
      <button class="admbtn" onclick={save} disabled={busy || (view === "form" && (parseError || errors.length > 0))}>{busy ? "saving…" : "Save"}</button>
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
