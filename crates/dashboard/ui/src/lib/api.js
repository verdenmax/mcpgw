/** GET a JSON endpoint; throws on non-2xx. */
export async function getJSON(path) {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path} -> ${r.status}`);
  return r.json();
}
