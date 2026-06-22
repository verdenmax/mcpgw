<script>
  import { getJSON } from "./api.js";
  import { refresh } from "./refresh.svelte.js";
  import Sparkline from "./Sparkline.svelte";
  // `window` 是全局名,别名为 win 避免遮蔽。sections: 逗号分隔的 spark/breakdown/leaders。
  let { window: win, sections = "spark", onpick } = $props();
  let data = $state(null);
  const shown = $derived(new Set(sections.split(",")));
  async function load() {
    const reqW = win; // 丢弃被新窗口取代的过期响应
    try {
      const d = await getJSON(`/api/activity?window=${win}`);
      if (reqW === win) data = d;
    } catch (_) { /* 次要数据,主错误 UI 归页面所有 */ }
  }
  $effect(() => { void win; refresh.tick; load(); });
</script>

{#if data}
  {#if shown.has("spark")}
    <div class="actpanel">
      <div class="actpanel-h">Activity · 最近 {Math.round(win / 60000)} 分钟</div>
      {#if data.total > 0}
        <Sparkline buckets={data.buckets} bucketMs={data.bucket_ms} {onpick} />
        <div class="spark-legend"><span>{data.total} calls</span>{#if data.errors}<span class="bad">{data.errors} errors</span>{/if}</div>
      {:else}
        <div class="muted">no activity yet</div>
      {/if}
    </div>
  {/if}

  {#if shown.has("breakdown") && data.by_error_kind.length}
    <div class="kindbar">
      {#each data.by_error_kind as k}<span class="tag"><span class="bad">{k.kind}</span> {k.count}</span>{/each}
    </div>
  {/if}

  {#if shown.has("leaders")}
    <div class="leadrow">
      <div class="lead">
        <div class="lead-h">最慢调用</div>
        {#each data.slowest as s}
          <a class="lead-li" href={`#/calls/${s.id}`}><span class="mono">{s.label}</span><span class="num bad">{s.latency_ms}ms</span></a>
        {/each}
        {#if !data.slowest.length}<div class="muted">—</div>{/if}
      </div>
      <div class="lead">
        <div class="lead-h">最忙工具</div>
        {#each data.busiest_tools as t}
          <a class="lead-li" href={`#/tools/${encodeURIComponent(t.name)}`}><span class="mono">{t.name}</span><span class="num">{t.count}</span></a>
        {/each}
        {#if !data.busiest_tools.length}<div class="muted">—</div>{/if}
      </div>
    </div>
  {/if}
{/if}
