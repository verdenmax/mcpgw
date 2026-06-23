<script>
  import { STRATEGIES, defaultSection } from "./configSchema.js";
  let { retrieval = $bindable() } = $props();
  $effect(() => {
    if ((retrieval?.strategy === "vector" || retrieval?.strategy === "hybrid") && !retrieval.vector) retrieval.vector = { model: "", api_key_env: "" };
    if (retrieval?.strategy === "subagent" && !retrieval.subagent) retrieval.subagent = { model: "", api_key_env: "" };
  });
</script>

{#if retrieval === undefined}
  <p class="muted">[retrieval] 段未配置（运行时按默认值）。</p>
  <button type="button" class="admbtn" onclick={() => (retrieval = defaultSection("retrieval"))}>+ 启用 [retrieval]</button>
{:else}
  <label class="cfg-field">strategy
    <select bind:value={retrieval.strategy}>{#each STRATEGIES as s}<option value={s}>{s}</option>{/each}</select>
  </label>
  <label class="cfg-field">top_k <input type="number" min="1" bind:value={retrieval.top_k} /></label>

  {#if (retrieval.strategy === "vector" || retrieval.strategy === "hybrid") && retrieval.vector}
    <fieldset class="cfg-sub"><legend>vector</legend>
      <label class="cfg-field">base_url <input bind:value={retrieval.vector.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">model <input bind:value={retrieval.vector.model} /></label>
      <label class="cfg-field">api_key_env <input bind:value={retrieval.vector.api_key_env} placeholder="环境变量名" /></label>
      <label class="cfg-field">dim <input type="number" min="1" bind:value={retrieval.vector.dim} /></label>
      <label class="cfg-field">timeout_ms <input type="number" min="1" bind:value={retrieval.vector.timeout_ms} /></label>
      <label class="cfg-field">batch_size <input type="number" min="1" bind:value={retrieval.vector.batch_size} /></label>
    </fieldset>
  {/if}
  {#if retrieval.strategy === "subagent" && retrieval.subagent}
    <fieldset class="cfg-sub"><legend>subagent</legend>
      <label class="cfg-field">base_url <input bind:value={retrieval.subagent.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">model <input bind:value={retrieval.subagent.model} /></label>
      <label class="cfg-field">api_key_env <input bind:value={retrieval.subagent.api_key_env} placeholder="环境变量名" /></label>
      <label class="cfg-field">timeout_ms <input type="number" min="1" bind:value={retrieval.subagent.timeout_ms} /></label>
      <label class="cfg-field">candidates <input type="number" min="1" bind:value={retrieval.subagent.candidates} /></label>
    </fieldset>
  {/if}
{/if}
