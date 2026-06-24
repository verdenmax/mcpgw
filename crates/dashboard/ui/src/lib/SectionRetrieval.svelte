<script>
  import { STRATEGIES, defaultSection } from "./configSchema.js";
  let { retrieval = $bindable() } = $props();
  $effect(() => {
    if ((retrieval?.strategy === "vector" || retrieval?.strategy === "hybrid") && !retrieval.vector) retrieval.vector = { model: "", api_key_env: "" };
    if (retrieval?.strategy === "subagent" && !retrieval.subagent) retrieval.subagent = { model: "", api_key_env: "" };
  });
  function onStrategy() {
    if (retrieval.strategy !== "vector" && retrieval.strategy !== "hybrid") delete retrieval.vector;
    if (retrieval.strategy !== "subagent") delete retrieval.subagent;
  }
</script>

{#if retrieval === undefined}
  <p class="muted">[retrieval] 段未配置（运行时按默认值）。</p>
  <button type="button" class="iconbtn" onclick={() => (retrieval = defaultSection("retrieval"))}>+ 启用 [retrieval]</button>
{:else}
  <label class="cfg-field">strategy
    <select bind:value={retrieval.strategy} onchange={onStrategy}>{#each STRATEGIES as s}<option value={s}>{s}</option>{/each}</select>
    <span class="cfg-hint">检索策略：bm25=纯词法召回（无需 key）、vector=向量语义、hybrid=词法+向量混合、subagent=智能体规划</span>
  </label>
  <label class="cfg-field">top_k
    <input type="number" min="1" bind:value={retrieval.top_k} />
    <span class="cfg-hint">每次检索返回给客户端的工具条数上限</span>
  </label>

  {#if (retrieval.strategy === "vector" || retrieval.strategy === "hybrid") && retrieval.vector}
    <div class="cfg-sub" role="group" aria-label="vector">
      <div class="cfg-sub-h">vector</div>
      <label class="cfg-field">model
        <input bind:value={retrieval.vector.model} />
        <span class="cfg-hint">向量化（embedding）模型名，如 text-embedding-3-small</span>
      </label>
      <label class="cfg-field">api_key_env
        <input bind:value={retrieval.vector.api_key_env} placeholder="环境变量名" />
        <span class="cfg-hint">存放 API key 的环境变量名（只填变量名，不填密钥本身）</span>
      </label>
      <label class="cfg-field">base_url <span class="cfg-q" title="向量化服务的 API 基地址；留空用内置默认" aria-label="向量化服务的 API 基地址；留空用内置默认">?</span><input bind:value={retrieval.vector.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">dim <span class="cfg-q" title="向量维度，需与所选模型匹配（可选）" aria-label="向量维度，需与所选模型匹配（可选）">?</span><input type="number" min="1" bind:value={retrieval.vector.dim} /></label>
      <label class="cfg-field">timeout_ms <span class="cfg-q" title="单次向量化请求的超时（毫秒）" aria-label="单次向量化请求的超时（毫秒）">?</span><input type="number" min="1" bind:value={retrieval.vector.timeout_ms} /></label>
      <label class="cfg-field">batch_size <span class="cfg-q" title="批量向量化时每批的条数" aria-label="批量向量化时每批的条数">?</span><input type="number" min="1" bind:value={retrieval.vector.batch_size} /></label>
    </div>
  {/if}
  {#if retrieval.strategy === "subagent" && retrieval.subagent}
    <div class="cfg-sub" role="group" aria-label="subagent">
      <div class="cfg-sub-h">subagent</div>
      <label class="cfg-field">model
        <input bind:value={retrieval.subagent.model} />
        <span class="cfg-hint">规划用 LLM 模型名</span>
      </label>
      <label class="cfg-field">api_key_env
        <input bind:value={retrieval.subagent.api_key_env} placeholder="环境变量名" />
        <span class="cfg-hint">存放 API key 的环境变量名（只填变量名）</span>
      </label>
      <label class="cfg-field">base_url <span class="cfg-q" title="LLM 服务 API 基地址；留空用默认" aria-label="LLM 服务 API 基地址；留空用默认">?</span><input bind:value={retrieval.subagent.base_url} placeholder="(默认)" /></label>
      <label class="cfg-field">timeout_ms <span class="cfg-q" title="单次规划请求超时（毫秒）" aria-label="单次规划请求超时（毫秒）">?</span><input type="number" min="1" bind:value={retrieval.subagent.timeout_ms} /></label>
      <label class="cfg-field">candidates <span class="cfg-q" title="每轮候选工具数（可选）" aria-label="每轮候选工具数（可选）">?</span><input type="number" min="1" bind:value={retrieval.subagent.candidates} /></label>
    </div>
  {/if}
{/if}
