// Shared formatting + navigation helpers (deduped from per-component copies).

/** Format an epoch-ms timestamp for display in the local timezone. */
export function when(ms) {
  return new Date(ms).toLocaleString();
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

/** Keyboard activation handler for role="button" rows: Enter/Space navigates to `hash`. */
export function rowKey(hash) {
  return (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      location.hash = hash;
    }
  };
}
