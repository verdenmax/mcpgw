// Tiny hash router: `#/<view>/<...params>` -> reactive { view, params }. Hash routing means the
// fragment is never sent to the server, so deep-link refresh only ever requests `/`.
function parse() {
  const raw = window.location.hash.replace(/^#\/?/, "");
  const parts = raw.split("/").filter(Boolean).map((p) => {
    try { return decodeURIComponent(p); } catch (_) { return p; }
  });
  return { view: parts[0] || "overview", params: parts.slice(1) };
}

export const route = $state(parse());

export function startRouter() {
  const update = () => {
    const r = parse();
    route.view = r.view;
    route.params = r.params;
  };
  window.addEventListener("hashchange", update);
  update();
  return () => window.removeEventListener("hashchange", update);
}
