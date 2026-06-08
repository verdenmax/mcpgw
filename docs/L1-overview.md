# L1 — mcpgw 概览

## 这是什么

**mcpgw** 是一个智能 MCP（Model Context Protocol）网关。其核心差异化能力是在**网关/代理层**实现
**渐进式工具发现（progressive tool discovery）**：把 N 个上游 MCP 服务器聚合起来，但只向客户端暴露
少量"元工具"，由网关在内部做工具检索与按需加载，从而避免"把上百个工具一次性塞给 LLM"导致的上下文
爆炸与选错工具。

本文档覆盖的范围是 **M0（检索核心 / Plan 1）**：项目的依赖最少、纯逻辑的检索内核。它本身可独立运行
（一个加载工具目录、做 BM25 检索的库 + CLI），并为后续 M1（活 MCP I/O 层）打好接口地基。

> 完整里程碑路线见 `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`。
> 设计依据见 `docs/superpowers/specs/2026-06-08-mcpgw-progressive-discovery-design.md`。

## 整体架构（M0）

Cargo **虚拟工作区**，四个 crate，职责单一、边界清晰：

```
                       ┌────────────────────────── mcpgw (bin) ──────────────────────────┐
                       │  clap CLI：search / get-details；装配 catalog + config + retrieval │
                       └───────────────┬─────────────────────┬───────────────────────────┘
                                       │                     │
              ┌────────────────────────▼──────┐   ┌──────────▼─────────────────────────┐
              │  retrieval                     │   │  config                            │
              │  RetrievalStrategy trait       │   │  Config / RetrievalConfig          │
              │  Bm25Strategy / build_strategy │   │  from_toml_str（[retrieval] 解析）  │
              └────────────────────────┬───────┘   └────────────────────────────────────┘
                                       │  (依赖 catalog 类型)
                       ┌───────────────▼───────────────┐
                       │  catalog                       │
                       │  ToolDef / Catalog / 命名空间    │
                       │  from_json_str（JSON 加载）      │
                       └────────────────────────────────┘
```

## crate 依赖关系（有意为之）

- `catalog` → 仅依赖 `serde`/`serde_json`，不依赖任何兄弟 crate。
- `retrieval` → **仅依赖 `catalog`**（不依赖 `config`/CLI）。`build_strategy(strategy: &str)` 故意接受
  字符串而非配置类型，保持核心排序 crate 的独立可复用性。
- `config` → 仅依赖 `serde`/`toml`/`thiserror`，**不反向依赖 `retrieval`**。
- `mcpgw`（bin）→ 唯一的集成者，依赖以上三者。

依赖方向无环：`mcpgw → {catalog, retrieval, config}`，`retrieval → catalog`。

## 数据流（M0 CLI）

```
读取 catalog JSON ──► Catalog::from_json_str ──► Catalog（命名空间注册表）
读取/默认 config  ──► Config::from_toml_str / default_from_empty
search 子命令：build_strategy(cfg.strategy) ──► strat.index(&catalog) ──► strat.search(query, top_k) ──► JSON
get-details 子命令：catalog.get(qualified_name) ──► 该工具完整 JSON
```

> 在最终形态（M1）里，这套"检索→详情→执行"会通过 `search_tools` / `get_tool_details` / `call_tool`
> 三个 MCP 元工具暴露给客户端；M0 先用 CLI 验证检索内核。

## 构建与测试

```bash
cargo build                 # 构建工作区（产出 target/debug/mcpgw）
cargo test                  # 全部测试（catalog 4 / config 6 / retrieval 5 + golden 1 / mcpgw cli 5）
cargo clippy --all-targets --all-features -- -D warnings   # 静态检查，零告警
cargo fmt --all             # 格式化
# 手动试用（需在工作区根目录运行，默认 --catalog tests/fixtures/tools.json）
./target/debug/mcpgw search "weather forecast"
./target/debug/mcpgw get-details github__create_issue
```

## 当前状态

- **M0（检索核心）✅ 已完成并合并到 `master`。** 21 测试绿、clippy 净。
- 下一步：**M1（活 MCP I/O 层）**，见路线图。

## 向下导航

各组件的职责与接口见 **L2**：
[catalog](./L2-components/catalog.md) · [retrieval](./L2-components/retrieval.md) ·
[config](./L2-components/config.md) · [mcpgw-cli](./L2-components/mcpgw-cli.md)
