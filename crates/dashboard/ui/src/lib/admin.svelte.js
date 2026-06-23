// Admin write access. The token lives ONLY in memory (cleared on refresh) — never localStorage.
import { postJSON } from "./api.js";

export const admin = $state({ token: "" });

/** POST to an admin endpoint with the in-memory Bearer token. Returns the Response. */
export function adminPost(path) {
  return postJSON(path, admin.token);
}

/** GET an admin endpoint with the in-memory Bearer token. Returns the raw Response. */
export function adminGet(path) {
  return fetch(path, { headers: admin.token ? { Authorization: `Bearer ${admin.token}` } : {} });
}

/** PUT a text body to an admin endpoint with the Bearer token. Returns the raw Response. */
export function adminPut(path, body) {
  return fetch(path, {
    method: "PUT",
    headers: admin.token ? { Authorization: `Bearer ${admin.token}` } : {},
    body,
  });
}
