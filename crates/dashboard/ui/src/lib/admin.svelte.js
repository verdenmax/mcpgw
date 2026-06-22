// Admin write access. The token lives ONLY in memory (cleared on refresh) — never localStorage.
import { postJSON } from "./api.js";

export const admin = $state({ token: "" });

/** POST to an admin endpoint with the in-memory Bearer token. Returns the Response. */
export function adminPost(path) {
  return postJSON(path, admin.token);
}
