import { parse, stringify } from "smol-toml";

/**
 * Parse raw TOML text into a model object (TOML-native keys: `upstream`, `api_key`).
 * Returns { ok: true, model } or { ok: false, error } — never throws.
 */
export function parseToml(raw) {
  try {
    return { ok: true, model: parse(raw) };
  } catch (e) {
    const msg = (e && typeof e.message === "string" && e.message) ? e.message : String(e);
    return { ok: false, error: msg || "Invalid TOML" };
  }
}

/**
 * Serialize a model back to canonical TOML text.
 * NOTE: comments / original formatting are NOT preserved (normalized output).
 */
export function stringifyToml(model) {
  return stringify(model);
}
