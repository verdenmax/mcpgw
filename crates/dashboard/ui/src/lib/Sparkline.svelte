<script>
  // 24 根堆叠柱：柱高 ∝ total/max；错误段红色叠在底部。inline SVG 用 currentColor/CSS 变量上色。
  let { buckets = [] } = $props();
  const W = 240, H = 46, GAP = 2, N = 24;
  const barW = (W - GAP * (N - 1)) / N;
  const max = $derived(Math.max(1, ...buckets.map((b) => b.total)));
  const totalCalls = $derived(buckets.reduce((a, b) => a + b.total, 0));
  const totalErr = $derived(buckets.reduce((a, b) => a + b.errors, 0));
  function when(t) { return new Date(t).toLocaleTimeString(); }
</script>

<svg class="spark" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img"
     aria-label={`${totalCalls} calls, ${totalErr} errors over the window`}>
  {#each buckets as b, i}
    {@const x = i * (barW + GAP)}
    {@const th = (b.total / max) * H}
    {@const eh = (b.errors / max) * H}
    <rect {x} y={H - th} width={barW} height={th} rx="1" fill="var(--accent)" opacity="0.85">
      <title>{when(b.t)} · {b.total} calls{b.errors ? `, ${b.errors} err` : ""}</title>
    </rect>
    {#if b.errors}
      <rect {x} y={H - eh} width={barW} height={eh} rx="1" fill="var(--danger)" />
    {/if}
  {/each}
</svg>
