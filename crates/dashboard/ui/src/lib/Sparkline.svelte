<script>
  // 24 根可交互 flex 柱：非零柱顶部显示计数、可点击 onpick(since, until)；零柱细基线（非交互）。
  // 柱体蓝紫渐变、底部红色错误段（占该柱高度的 errors/total）。纯 DOM，无依赖、无 raw-html 注入。
  let { buckets = [], bucketMs = 0, onpick } = $props();
  const max = $derived(Math.max(1, ...buckets.map((b) => b.total)));
  const totalCalls = $derived(buckets.reduce((a, b) => a + b.total, 0));
  const totalErr = $derived(buckets.reduce((a, b) => a + b.errors, 0));
  function title(b) {
    return `${new Date(b.t).toLocaleTimeString()} · ${b.total} calls${b.errors ? `, ${b.errors} err` : ""}`;
  }
</script>

<div class="sparkbars" role="group" aria-label={`activity: ${totalCalls} calls, ${totalErr} errors over the window`}>
  {#each buckets as b}
    {#if b.total > 0}
      <button class="barcol barbtn" title={title(b)} onclick={() => onpick?.(b.t, b.t + bucketMs - 1)}>
        <span class="barnum">{b.total}</span>
        <span class="bar" style="height:{(b.total / max) * 100}%">
          {#if b.errors}<span class="barerr" style="height:{(b.errors / b.total) * 100}%"></span>{/if}
        </span>
      </button>
    {:else}
      <span class="barcol"><span class="bar0"></span></span>
    {/if}
  {/each}
</div>
