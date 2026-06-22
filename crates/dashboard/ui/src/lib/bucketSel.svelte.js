// Overview 点 sparkline 柱后暂存所选绝对窗（since/until, epoch ms）；Calls 初始化时消费一次。
// 纯内存、无持久化。Svelte 5 universal reactivity（.svelte.js 里的 $state）。
export const pendingBucket = $state({ since: null, until: null });
