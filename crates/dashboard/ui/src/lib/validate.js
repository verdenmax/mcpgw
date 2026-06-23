import { STRATEGIES, TRANSPORTS } from "./configSchema.js";

/**
 * Field-level validation of a config model. Pure function, never throws.
 * Returns an array of { path, msg } (empty = valid). The BACKEND remains the
 * authority for env-resolution and full structural validation at Save time.
 */
export function validateModel(model) {
  model = model || {};
  const errors = [];
  const push = (path, msg) => errors.push({ path, msg });

  const r = model.retrieval;
  if (r) {
    if (r.strategy !== undefined && !STRATEGIES.includes(r.strategy))
      push("retrieval.strategy", `strategy 必须是 ${STRATEGIES.join(" / ")}`);
    if (r.top_k !== undefined && (!Number.isInteger(r.top_k) || r.top_k < 1))
      push("retrieval.top_k", "top_k 必须是 ≥1 的整数");
    if (r.strategy === "vector" || r.strategy === "hybrid") requireSub(push, "retrieval.vector", r.vector);
    if (r.strategy === "subagent") requireSub(push, "retrieval.subagent", r.subagent);
  }

  const s = model.server;
  if (s && s.http && Array.isArray(s.http.api_key)) {
    s.http.api_key.forEach((k, i) => {
      if (!k.name || !k.name.trim()) push(`server.http.api_key[${i}].name`, "api_key name 必填");
      if (!k.env || !k.env.trim()) push(`server.http.api_key[${i}].env`, "api_key env 必填");
    });
  }

  const ups = model.upstream;
  if (Array.isArray(ups)) {
    const seen = new Set();
    ups.forEach((u, i) => {
      const base = `upstream[${i}]`;
      if (!u.name || !u.name.trim()) push(`${base}.name`, "name 必填");
      else {
        if (u.name.includes("__")) push(`${base}.name`, 'name 不能包含 "__"');
        if (/^_|_$/.test(u.name)) push(`${base}.name`, 'name 不能以 "_" 开头或结尾');
        if (seen.has(u.name)) push(`${base}.name`, `name "${u.name}" 重复`);
        seen.add(u.name);
      }
      if (u.call_timeout_ms !== undefined && (!Number.isInteger(u.call_timeout_ms) || u.call_timeout_ms < 1))
        push(`${base}.call_timeout_ms`, "call_timeout_ms 必须是 ≥1 的整数");
      if (!TRANSPORTS.includes(u.transport))
        push(`${base}.transport`, `transport 必须是 ${TRANSPORTS.join(" / ")}`);
      else if (u.transport === "stdio") {
        if (!u.command || !u.command.trim()) push(`${base}.command`, "stdio 上游 command 必填");
      } else if (u.transport === "http") {
        if (!u.url || !u.url.trim()) push(`${base}.url`, "http 上游 url 必填");
        if (u.headers) {
          for (const [hk, hv] of Object.entries(u.headers)) {
            if (hk && (typeof hv !== "string" || !hv.trim())) push(`${base}.headers.${hk}`, "header 值（env 名）必填");
          }
        }
      }
    });
  }

  return errors;
}

function requireSub(push, path, sub) {
  if (!sub || !sub.model || !sub.model.trim()) push(`${path}.model`, "model 必填");
  if (!sub || !sub.api_key_env || !sub.api_key_env.trim()) push(`${path}.api_key_env`, "api_key_env 必填");
}
