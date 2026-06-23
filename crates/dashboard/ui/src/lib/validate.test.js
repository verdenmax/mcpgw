import { test, expect } from "vitest";
import { validateModel } from "./validate.js";

test("empty model (all sections omitted) is valid", () => {
  expect(validateModel({})).toEqual([]);
});

test("valid model has no errors", () => {
  const m = {
    retrieval: { strategy: "bm25", top_k: 10 },
    upstream: [{ name: "mock", transport: "stdio", command: "/bin/x", call_timeout_ms: 30000 }],
  };
  expect(validateModel(m)).toEqual([]);
});

test("strategy out of enum", () => {
  const e = validateModel({ retrieval: { strategy: "bogus", top_k: 10 } });
  expect(e.some((x) => x.path === "retrieval.strategy")).toBe(true);
});

test("top_k must be >=1 integer", () => {
  const e = validateModel({ retrieval: { strategy: "bm25", top_k: 0 } });
  expect(e.some((x) => x.path === "retrieval.top_k")).toBe(true);
});

test("vector strategy requires vector.model + api_key_env", () => {
  const e = validateModel({ retrieval: { strategy: "vector", top_k: 5, vector: {} } });
  expect(e.some((x) => x.path === "retrieval.vector.model")).toBe(true);
  expect(e.some((x) => x.path === "retrieval.vector.api_key_env")).toBe(true);
});

test("upstream name cannot contain __ and must be unique", () => {
  const e = validateModel({ upstream: [
    { name: "a__b", transport: "stdio", command: "/x" },
    { name: "dup", transport: "stdio", command: "/x" },
    { name: "dup", transport: "stdio", command: "/x" },
  ]});
  expect(e.some((x) => x.path === "upstream[0].name" && /__/.test(x.msg))).toBe(true);
  expect(e.some((x) => x.path === "upstream[2].name" && /重复/.test(x.msg))).toBe(true);
});

test("stdio requires command, http requires url", () => {
  const e = validateModel({ upstream: [
    { name: "s", transport: "stdio" },
    { name: "h", transport: "http" },
  ]});
  expect(e.some((x) => x.path === "upstream[0].command")).toBe(true);
  expect(e.some((x) => x.path === "upstream[1].url")).toBe(true);
});

test("transport out of enum", () => {
  const e = validateModel({ upstream: [{ name: "x", transport: "grpc" }] });
  expect(e.some((x) => x.path === "upstream[0].transport")).toBe(true);
});

test("call_timeout_ms must be >=1 integer", () => {
  const e = validateModel({ upstream: [{ name: "x", transport: "stdio", command: "/x", call_timeout_ms: 0 }] });
  expect(e.some((x) => x.path === "upstream[0].call_timeout_ms")).toBe(true);
});

test("upstream name required when blank/missing", () => {
  const e = validateModel({ upstream: [{ transport: "stdio", command: "/x" }] });
  expect(e.some((x) => x.path === "upstream[0].name")).toBe(true);
});

test("upstream name cannot start or end with _", () => {
  const e = validateModel({ upstream: [{ name: "_foo", transport: "stdio", command: "/x" }] });
  expect(e.some((x) => x.path === "upstream[0].name")).toBe(true);
});

test("hybrid strategy also requires vector.model + api_key_env", () => {
  const bad = validateModel({ retrieval: { strategy: "hybrid", top_k: 5, vector: {} } });
  expect(bad.some((x) => x.path === "retrieval.vector.model")).toBe(true);
  expect(bad.some((x) => x.path === "retrieval.vector.api_key_env")).toBe(true);
  const ok = validateModel({ retrieval: { strategy: "hybrid", top_k: 5, vector: { model: "m", api_key_env: "K" } } });
  expect(ok.some((x) => x.path.startsWith("retrieval.vector"))).toBe(false);
});

test("subagent strategy requires subagent.model + api_key_env", () => {
  const e = validateModel({ retrieval: { strategy: "subagent", top_k: 5, subagent: {} } });
  expect(e.some((x) => x.path === "retrieval.subagent.model")).toBe(true);
  expect(e.some((x) => x.path === "retrieval.subagent.api_key_env")).toBe(true);
});
