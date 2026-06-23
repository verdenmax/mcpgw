import { parse, stringify } from "smol-toml";

/** Parse raw TOML into a model. Returns { ok:true, model } or { ok:false, error } — never throws. */
export function parseToml(raw) {
  try {
    return { ok: true, model: parse(raw) };
  } catch (e) {
    const msg = (e && typeof e.message === "string" && e.message) ? e.message : String(e);
    return { ok: false, error: msg || "Invalid TOML" };
  }
}

/**
 * Deep-copy a model dropping entries that can't/shouldn't serialize: null/undefined values,
 * empty-string values (cleared optional fields), and empty keys (blank header rows). This keeps
 * smol-toml happy (it can't serialize null) and prevents blank `"" = ""` header rows or cleared
 * optionals from being persisted (they fall back to backend defaults instead).
 */
export function pruneModel(value) {
  if (Array.isArray(value)) return value.map(pruneModel);
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      if (k === "" || v === null || v === undefined || v === "") continue;
      const pv = pruneModel(v);
      if (pv !== null && typeof pv === "object" && !Array.isArray(pv) && Object.keys(pv).length === 0) continue; // drop empty sub-tables
      out[k] = pv;
    }
    return out;
  }
  return value;
}

/** Serialize a model to canonical TOML (comments NOT preserved; null/undefined/empty pruned). */
export function stringifyToml(model) {
  return stringify(pruneModel(model));
}
