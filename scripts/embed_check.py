#!/usr/bin/env python3
"""Sanity-check an OpenAI-compatible /embeddings endpoint against the tool catalog.

Reads env: OPENAI_API_KEY (required), MCPGW_EMBED_BASE_URL (default OpenAI),
MCPGW_EMBED_MODEL (default text-embedding-3-small). Prints cosine ranking of tools for a
few semantic queries — manual inspection step before trusting the Rust integration.
"""
import json, math, os, sys, urllib.request

BASE = os.environ.get("MCPGW_EMBED_BASE_URL", "https://api.openai.com/v1").rstrip("/")
MODEL = os.environ.get("MCPGW_EMBED_MODEL", "text-embedding-3-small")
KEY = os.environ.get("OPENAI_API_KEY")
if not KEY:
    sys.exit("set OPENAI_API_KEY (and optionally MCPGW_EMBED_BASE_URL / MCPGW_EMBED_MODEL)")

TOOLS = {
    "slack__post_message": "Send a chat message to a Slack channel",
    "weather__get_forecast": "Get the weather forecast for a location",
    "github__create_issue": "Create a new issue in a GitHub repository",
    "filesystem__write_file": "Write contents to a file on disk",
}
QUERIES = ["communicate with my team", "will it rain tomorrow", "report a bug"]

def embed(texts):
    body = json.dumps({"model": MODEL, "input": texts}).encode()
    req = urllib.request.Request(
        f"{BASE}/embeddings", data=body,
        headers={"Authorization": f"Bearer {KEY}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req) as r:
        data = json.load(r)["data"]
    return [d["embedding"] for d in sorted(data, key=lambda d: d["index"])]

def cos(a, b):
    dot = sum(x*y for x, y in zip(a, b))
    na = math.sqrt(sum(x*x for x in a)); nb = math.sqrt(sum(y*y for y in b))
    return dot / (na*nb)

names = list(TOOLS)
tvecs = embed([TOOLS[n] for n in names])
for q, qv in zip(QUERIES, embed(QUERIES)):
    ranked = sorted(((cos(qv, tv), n) for n, tv in zip(names, tvecs)), reverse=True)
    print(f"\nQUERY: {q!r}")
    for score, n in ranked:
        print(f"  {score:.3f}  {n}")
