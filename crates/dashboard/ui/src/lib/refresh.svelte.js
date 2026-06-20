// Central auto-refresh controller. One interval bumps `tick`; every page re-fetches on `tick`
// change (instead of owning its own setInterval), so the header can pause/resume and trigger a
// manual refresh globally. `at` records the last refresh time for the "updated Ns ago" label.
export const refresh = $state({ paused: false, tick: 0, at: Date.now() });

const INTERVAL_MS = 3000;

/** Start the global ticker. Call once (App.onMount); returns a teardown that clears the interval. */
export function startRefresh() {
  const t = setInterval(() => {
    if (!refresh.paused) {
      refresh.tick++;
      refresh.at = Date.now();
    }
  }, INTERVAL_MS);
  return () => clearInterval(t);
}

/** Force an immediate refresh of every page (also works while paused). */
export function refreshNow() {
  refresh.tick++;
  refresh.at = Date.now();
}

/** Toggle auto-refresh on/off. */
export function togglePause() {
  refresh.paused = !refresh.paused;
}
