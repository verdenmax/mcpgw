import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Output to dist/ (committed, embedded by rust-embed). base '/' so assets resolve at /assets/*
// which the Rust static handler serves from the embedded dist/.
export default defineConfig({
  plugins: [svelte()],
  base: "/",
  build: { outDir: "dist", emptyOutDir: true },
});
