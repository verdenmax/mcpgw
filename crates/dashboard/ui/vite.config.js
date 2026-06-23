/// <reference types="vitest/config" />
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Output to dist/ (committed, embedded by rust-embed). base '/' so assets resolve at /assets/*
// which the Rust static handler serves from the embedded dist/.
export default defineConfig({
  plugins: [svelte()],
  base: "/",
  build: { outDir: "dist", emptyOutDir: true },
  // vitest: only the pure-JS helpers (toml.js / validate.js) are unit-tested, so a node
  // environment is enough (no jsdom / Svelte component rendering).
  test: { environment: "node", include: ["src/**/*.test.js"] },
});
