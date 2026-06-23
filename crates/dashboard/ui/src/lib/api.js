/** GET a JSON endpoint; throws on non-2xx. */
export async function getJSON(path) {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path} -> ${r.status}`);
  return r.json();
}

/** POST with an optional Bearer token; returns the raw Response (caller inspects status). */
export async function postJSON(path, token) {
  return fetch(path, {
    method: "POST",
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
}
