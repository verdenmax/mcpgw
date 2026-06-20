// Shared formatting + navigation helpers (deduped from per-component copies).

/** Format an epoch-ms timestamp for display in the local timezone. */
export function when(ms) {
  return new Date(ms).toLocaleString();
}

/** Compact relative age of an epoch-ms timestamp, e.g. "5s ago", "3m ago", "2h ago", "4d ago". */
export function ago(ms) {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.round(h / 24)}d ago`;
}

/** Pretty-print a JSON string; fall back to the raw string if it isn't valid JSON. */
export function pretty(s) {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch (_) {
    return s;
  }
}

/** Navigate the hash router to `hash` (e.g. "#/calls/12"). */
export function go(hash) {
  location.hash = hash;
}
