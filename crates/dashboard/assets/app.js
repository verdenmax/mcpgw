const REFRESH_MS = 3000;
const $ = (sel) => document.querySelector(sel);

async function j(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(url + " -> " + r.status);
  return r.json();
}

function card(label, value) {
  return `<div class="card"><div class="muted">${label}</div><div class="v">${value}</div></div>`;
}

async function refresh() {
  try {
    const ov = await j("/api/overview");
    $("#uptime").textContent = "up " + ov.uptime_secs + "s · strategy " + ov.strategy;
    $("#overview").innerHTML =
      card("upstreams", ov.upstreams_connected + "/" + ov.upstreams_total) +
      card("tools", ov.tools_total) +
      card("calls", ov.total_calls) +
      card("skipped", ov.last_rebuild_skipped);

    const ups = await j("/api/upstreams");
    $("#upstreams tbody").innerHTML = ups.map((u) =>
      `<tr><td>${u.name}</td><td>${u.transport}</td>` +
      `<td><span class="badge ${u.status}">${u.status}</span>${u.reason ? " " + u.reason : ""}</td>` +
      `<td>${u.tools}</td><td>${u.calls}</td><td>${u.errors}</td></tr>`).join("");

    const m = await j("/api/metrics");
    const maxCalls = Math.max(1, ...m.per_meta_tool.map((x) => x.calls));
    $("#metrics").innerHTML = m.per_meta_tool.map((x) =>
      `<div><b>${x.meta_tool}</b> calls ${x.calls} · err ${x.errors} · p50 ${x.p50_ms}ms · p95 ${x.p95_ms}ms` +
      `<div class="bar"><span style="width:${(100 * x.calls / maxCalls).toFixed(0)}%"></span></div></div>`).join("");

    const src = $("#trace-source").value;
    const t = await j("/api/traces?limit=50&source=" + src);
    $("#traces").innerHTML = t.history_unavailable
      ? `<p class="muted">history unavailable (enable [dashboard].trace_path)</p>`
      : t.traces.map((r) =>
          `<div class="trace"><div class="q">${escapeHtml(r.query)}</div>` +
          r.results.map((h) => `<span class="hit">${h.name} (${h.score.toFixed(2)})</span>`).join(" · ") +
          `</div>`).join("");
  } catch (e) {
    console.error(e);
  }
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

refresh();
setInterval(refresh, REFRESH_MS);
