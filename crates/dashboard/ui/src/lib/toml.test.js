import { test, expect } from "vitest";
import { parseToml, stringifyToml } from "./toml.js";
import { pruneModel } from "./toml.js";

const SAMPLE = `[retrieval]
strategy = "bm25"
top_k = 10

[retrieval.vector]
model = "e5"
api_key_env = "VK"
dim = 768

[server]
stdio = false

[server.http]
enabled = true
bind = "127.0.0.1:8970"
path = "/mcp"

[[server.http.api_key]]
name = "a"
env = "K"

[[upstream]]
name = "mock"
transport = "stdio"
command = "/bin/mock"
args = ["--x"]
env_passthrough = ["PATH", "HOME"]
call_timeout_ms = 30000

[[upstream]]
name = "remote"
transport = "http"
url = "https://x/mcp"
bearer_env = "TKN"
`;

test("parseToml returns ok model with TOML-native keys", () => {
  const r = parseToml(SAMPLE);
  expect(r.ok).toBe(true);
  expect(r.model.retrieval.strategy).toBe("bm25");
  expect(r.model.retrieval.top_k).toBe(10);
  expect(r.model.retrieval.vector.dim).toBe(768);
  expect(r.model.server.http.api_key[0].env).toBe("K");
  expect(r.model.upstream).toHaveLength(2);
  expect(r.model.upstream[0].transport).toBe("stdio");
  expect(r.model.upstream[1].url).toBe("https://x/mcp");
});

test("round-trip parse→stringify→parse is semantically equal", () => {
  const a = parseToml(SAMPLE);
  const out = stringifyToml(a.model);
  const b = parseToml(out);
  expect(b.ok).toBe(true);
  expect(b.model).toEqual(a.model);
});

test("parseToml returns structured error on invalid TOML", () => {
  const r = parseToml("this is = = not toml");
  expect(r.ok).toBe(false);
  expect(typeof r.error).toBe("string");
  expect(r.error.length).toBeGreaterThan(0);
});

test("stringifyToml on empty model yields empty-ish TOML that re-parses", () => {
  const out = stringifyToml({});
  expect(parseToml(out)).toEqual({ ok: true, model: {} });
});

test("round-trip of a form-built model: subagent, http headers map, empty lists", () => {
  const model = {
    retrieval: { strategy: "subagent", top_k: 8,
      subagent: { base_url: "http://x", model: "m", api_key_env: "K", candidates: 5 } },
    upstream: [
      { name: "mock", transport: "stdio", command: "/bin/mock", args: [], env_passthrough: [] },
      { name: "remote", transport: "http", url: "https://x/mcp", bearer_env: "TKN",
        headers: { "X-Tenant": "ENV_T", "X-Trace": "ENV_R" } },
    ],
  };
  const round = parseToml(stringifyToml(model));
  expect(round.ok).toBe(true);
  expect(round.model).toEqual(model);
});

test("pruneModel drops null/undefined so smol-toml can serialize", () => {
  const m = { retrieval: { strategy: "bm25", top_k: null }, upstream: [{ name: "x", transport: "stdio", command: "/x", url: undefined }] };
  const p = pruneModel(m);
  expect("top_k" in p.retrieval).toBe(false);
  expect("url" in p.upstream[0]).toBe(false);
  expect(p.upstream[0].name).toBe("x");
});

test("pruneModel drops empty-string values and empty keys (e.g. blank header rows / cleared optionals)", () => {
  const m = { retrieval: { strategy: "bm25", top_k: 8, vector: { model: "m", api_key_env: "K", base_url: "" } },
    upstream: [{ name: "u", transport: "http", url: "https://x", headers: { "": "", "X-Real": "ENV" } }] };
  const p = pruneModel(m);
  expect("base_url" in p.retrieval.vector).toBe(false);   // cleared optional dropped
  expect("" in p.upstream[0].headers).toBe(false);         // blank header key dropped
  expect(p.upstream[0].headers["X-Real"]).toBe("ENV");     // real header kept
});

test("stringifyToml tolerates null fields via prune", () => {
  const out = stringifyToml({ retrieval: { strategy: "bm25", top_k: 10, vector: null } });
  expect(out).toContain("strategy");
  expect(out).not.toContain("vector");
});
