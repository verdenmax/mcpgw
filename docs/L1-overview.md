# L1 — mcpgw 概览

## 这是什么

**mcpgw** 是一个智能 MCP（Model Context Protocol）网关。其核心差异化能力是在**网关/代理层**实现
**渐进式工具发现（progressive tool discovery）**：把 N 个上游 MCP 服务器聚合起来，但只向客户端暴露
少量"元工具"，由网关在内部做工具检索与按需加载，从而避免"把上百个工具一次性塞给 LLM"导致的上下文
爆炸与选错工具。

本文档的基线范围是 **M0（检索核心 / Plan 1）**：项目的依赖最少、纯逻辑的检索内核。它本身可独立运行
（一个加载工具目录、做 BM25 检索的库 + CLI），并为后续 M1（活 MCP I/O 层）打好接口地基。

**M1 已完成**：上游 I/O 层 `upstream`（M1-A）；网关元工具逻辑 `metatools` 与快照状态/重建层 `gateway`
（**M1-B.1**）；下游 MCP 服务 `downstream` 与 eager-connect/`serve`（**M1-B.2**）；**HTTP 双向传输 + 静态
API-Key 鉴权（M1-C）**。`mcpgw serve` 现可并发起一个活的 **stdio 与/或 Streamable HTTP** MCP 网关，并能聚合
**远程 HTTP 上游** MCP server。

> 完整里程碑路线见 `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`。
> 设计依据见 `docs/superpowers/specs/2026-06-08-mcpgw-progressive-discovery-design.md`
> 与 `docs/superpowers/specs/2026-06-11-mcpgw-m1c-http-auth-design.md`（M1-C HTTP/鉴权）。

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

## M1 新增 crate：`upstream`（M1-A，已完成）

活的上游 MCP I/O 层，是 M1 的第一块拼图：

```
              ┌──────────────────────── upstream ────────────────────────┐
              │  UpstreamHandle（rmcp client：connect/ingest_into/call_tool）│
              │  UpstreamRegistry（server name -> Arc<Handle>）             │
              │  mapping（Tool → 命名空间 ToolDef，含冲突检测）              │
              └───────────────────────────┬──────────────────────────────┘
                            (摄取进 catalog) │ (依赖 catalog 类型)
                                            ▼
                                        catalog
```

- 依赖 **`rmcp`**（1.7，活的 MCP client/server）+ **`catalog`**（摄取目标类型），另有 `tokio`/`thiserror`/`tracing`。
- 把 N 个上游服务器的工具聚合进 `catalog` 命名空间（`{server}__{name}`），并把元工具层的 `call_tool` 路由回对应上游。
- 被未来的 **gateway（M1-B）** 使用；网关元工具（`search_tools`/`get_tool_details`/`call_tool`）与下游服务（M1-C）尚未实现。
- 接口/细节见 L2/L3/L4：[upstream](./L2-components/upstream.md)。

## M1-B.1 新增 crate：`metatools` + `gateway`（已完成）

网关层的逻辑与状态两块拼图：

```
        ┌──────────────────────────── gateway ────────────────────────────┐
        │  GatewayState：Arc<ArcSwap<GatewaySnapshot>>（读无锁）             │
        │   + UpstreamRegistry + strategy_name + rebuild_lock(tokio::Mutex) │
        │  rebuild_snapshot：ingest → build → 原子 swap（串行化、错误隔离）   │
        └───────┬───────────────────────────────────┬──────────────────────┘
                │ 持有/重建                          │ call_tool 路由
        ┌───────▼───────────────────────┐   ┌───────▼────────────┐
        │  metatools                     │   │  upstream          │
        │  GatewaySnapshot（catalog+策略）│   │  UpstreamRegistry  │
        │  search_tools/get_tool_details │   │  UpstreamHandle    │
        │  /call_tool · ToolSummary      │   └────────┬───────────┘
        │  MetaError                     │            │ (摄取/转发)
        └───────┬────────────────────────┘            ▼
                │ (依赖 catalog/retrieval 类型)      catalog
                ▼
            catalog + retrieval
```

- `metatools` → 依赖 `catalog`/`retrieval`/`upstream`/`rmcp`：在不可变 `GatewaySnapshot` 上提供三个元工具函数；
  `call_tool` **经 catalog 查 `(server, tool)` 路由**（绝不拆 `__`）。
- `gateway` → 依赖 `metatools`/`catalog`/`retrieval`/`upstream` + `arc-swap`/`tokio`：用 `ArcSwap` 持有快照
  （读无锁），`rebuild_snapshot` 用 **build-then-swap** 重建并经 `tokio::sync::Mutex` 串行化（防陈旧快照、单上游失败隔离）。
- 被下游 MCP 服务（**M1-B.2**）使用：把元工具暴露为 MCP 工具、做 eager-connect（`connect_all`/`serve`）。
- 接口/细节见 L2/L3/L4：[metatools](./L2-components/metatools.md) · [gateway](./L2-components/gateway.md)。

## M1-B.2 新增 crate：`downstream` + 活网关装配（已完成）

最后一块拼图：把元工具暴露为真正的 MCP 服务，并把上游 eager-connect、list_changed 热刷新接起来。

```
        MCP 客户端 ──stdio──► ┌──────────────── downstream ────────────────┐
                              │  GatewayServer: rmcp ServerHandler          │
                              │  list_tools = 固定 3 元工具（恒定）          │
                              │  call_tool 分派 → metatools 三函数           │
                              └──────────────────┬──────────────────────────┘
                                                 │ 读快照 / 取注册表
        ┌──────────────── mcpgw serve（装配） ───▼──────────────────────────┐
        │  prepare_state: connect_all(上游, trigger) → 初始 rebuild_snapshot  │
        │  spawn run_rebuild_worker(state, rx)  ◄── RebuildTrigger（mpsc）     │
        │  GatewayServer.serve(stdio()) → waiting() → 收尾 shutdown 上游       │
        └───────┬──────────────────────────────────────┬────────────────────┘
                │ eager-connect / 转发                  │ list_changed 触发重建
        ┌───────▼───────────┐               ┌──────────▼──────────────────────┐
        │  upstream::connect │               │  gateway                         │
        │  connect_all       │               │  rebuild_snapshot（并发摄取+超时）│
        │  (降级启动+env白名单)│              │  run_rebuild_worker（合并突发）   │
        └────────────────────┘               └──────────────────────────────────┘
```

- `downstream` → 依赖 `gateway`/`metatools`/`rmcp`：`GatewayServer` 实现 rmcp `ServerHandler`，
  `list_tools` **恒返回 3 个元工具**（故 `get_info` 不声明 `list_changed`——元工具集合恒定），`call_tool` 分派到
  `metatools`（`MetaError`→`isError`，未知名→`McpError`）。
- **活网关链路**（`mcpgw serve`）：`upstream::connect::connect_all` eager-connect 所有上游（**降级启动**：连不上
  只记录不阻断；env **allow-list**：子进程默认清空环境）→ 初始 `rebuild_snapshot` → spawn
  `gateway::run_rebuild_worker`（上游 `tools/list_changed` → `RebuildTrigger` → 合并突发为单次重建）→
  `GatewayServer` over stdio。**重建并发摄取 + per-ingest 超时**，hung/慢上游被隔离进 `skipped`，不拖死重建。
- **日志走 stderr**（stdout 留给 MCP 协议帧）。
- 接口/细节见 L2/L3/L4：[downstream](./L2-components/downstream.md)。

## M1-C 新增：HTTP 双向传输 + 静态 API-Key 鉴权（已完成）

补齐网关的 HTTP 双向能力与静态鉴权，使其既能被远程客户端访问，又能聚合远程 HTTP 上游——三个元工具与 stdio 完全
一致，只是多了 HTTP transport 与鉴权层。

```
   远程 MCP 客户端 ──HTTP──► ┌──────── downstream::http (axum) ────────┐
                            │  StreamableHttpService + nest_service     │
                            │  + Bearer 鉴权层（常量时间比较 / 401）    │
                            └────────────────────┬─────────────────────┘
                                                 ▼
   本地 MCP 客户端 ──stdio──────────► GatewayServer（3 元工具 · rmcp ServerHandler）
                                       （stdio 直连，不经 axum / 鉴权层）
   ┌──────────── mcpgw serve（并发装配，共享 Arc<GatewayState>）───────────▼──────────┐
   │  fail-fast 解析所有 env 引用的密钥 → 预绑定 HTTP listener →                        │
   │  tokio::select! over { stdio waiting() · axum::serve · ctrl_c } → 统一关闭         │
   └───────┬───────────────────────────────────────────────┬───────────────────────────┘
           │ eager-connect（按 transport 分派）             │ call_tool 路由
   ┌───────▼────────────────────────────────┐     ┌────────▼──────────────────────┐
   │  upstream::connect                       │     │  gateway / metatools           │
   │  connect_stdio_upstream（stdio 子进程）  │     │  GatewaySnapshot · call_tool   │
   │  connect_http_upstream（远程 HTTP MCP）  │     └────────────────────────────────┘
   │   复用泛型 connect_with_trigger 管线      │
   └──────────────────────────────────────────┘
```

- **下游 HTTP**：`downstream::http::build_router` 用 rmcp `StreamableHttpService` 把 `GatewayServer` 经
  `nest_service` 挂进 axum，默认绑 `127.0.0.1:8970`、路径 `/mcp`。配置 ≥1 个 API-Key 时叠加 Bearer 鉴权层
  （多 key、**常量时间比较**；缺失/错误 → **401**，不回显期望值）；keyset 为空则放行（依赖 localhost 绑定）。
- **上游 HTTP**：`UpstreamTransport::Http { url, bearer_env, headers }` 连接远程 HTTP MCP；`bearer_env` 持
  **原始 token**（rmcp 在线路上自动加 `Bearer ` 前缀），`headers` 是「头名 → env 变量名」内联表。HTTP 上游
  **复用与 stdio 同一条泛型连接/超时/list_changed 管线**，连接失败同样降级隔离。
- **进程模型**：`serve` 按配置并发跑 stdio 与/或 HTTP，共享同一 `Arc<GatewayState>`，经
  `tokio::select!` over `{stdio waiting()、axum::serve、ctrl_c}` 统一关闭；**至少须启用一种传输**。
- **Fail-fast**：所有 env 引用的密钥/头值在启动时解析校验，缺失即报错并指明字段名与 env 变量名（**绝不泄露值**）。
- **继续延后**：完整 OAuth/DCR/反向代理正确性 → M3；运行时热吊销/增删 API-Key → M4；超时主动向上游发
  `notifications/cancelled` → 仍延后（与 HTTP/鉴权正交，drop in-flight future 在 Rust 里已安全）。
- 接口/细节见 L2/L3/L4：[config](./L2-components/config.md) · [downstream](./L2-components/downstream.md) ·
  [upstream](./L2-components/upstream.md) · [downstream/http.rs](./L4-api/downstream-http.md) ·
  [upstream/connect.rs](./L4-api/upstream-connect.md)。

## 传输能力一览

| 方向 | stdio | HTTP（Streamable HTTP） |
|------|-------|--------------------------|
| **上游**（连接被聚合的 MCP server） | ✅ 子进程（`command`/`args` + env allow-list） | ✅ 远程 `url` + 静态鉴权（`bearer_env` 原始 token、`headers` 头名→env） |
| **下游**（向客户端暴露 3 个元工具） | ✅ `serve` over stdio | ✅ 默认 `127.0.0.1:8970` `/mcp` + 多 key Bearer 鉴权 |

> 下游 stdio 与 HTTP **可并发同时启用**（共享一份 `Arc<GatewayState>`）；至少启用一种。

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
cargo test --all-features   # 全部测试（84 个：catalog 4 / config 19 / retrieval 5 + golden 1 /
                            #   mcpgw main 5 + cli 5 / upstream 11 + 集成 10 + http_connect 1 /
                            #   metatools 3 + call_tool 4 / gateway 1 + rebuild 6 /
                            #   downstream 1 + e2e(stdio) 5 + e2e(http) 3）
                            # 注：upstream 集成测试、mock-stdio 二进制与 HTTP e2e 需 testkit feature，故用 --all-features
cargo clippy --all-targets --all-features -- -D warnings   # 静态检查，零告警
cargo fmt --all             # 格式化
# 手动试用（search/get-details 需在工作区根目录运行，默认 --catalog tests/fixtures/tools.json）
./target/debug/mcpgw search "weather forecast"
./target/debug/mcpgw get-details github__create_issue
# 起活的 MCP 网关（按配置并发跑 stdio 与/或 HTTP；日志走 stderr，stdout 是 MCP 协议帧）：
./target/debug/mcpgw --config mcpgw.toml serve
```

## 当前状态

- **M0（检索核心）✅ 已完成并合并到 `master`。** 21 测试绿、clippy 净。
- **M1（活 MCP I/O 层）✅ 已完成**：
  - **M1-A（`upstream`）✅ 已完成** —— rmcp client 连接、工具摄取、`call_tool` 转发（带每调用超时）、连接注册表；
    含 `testkit` 内存 mock 与门控集成测试。
  - **M1-B.1（`metatools` + `gateway`）✅ 已完成** —— 三个元工具函数 over 不可变 `GatewaySnapshot`、`ArcSwap`
    快照状态 + `rebuild_snapshot`（build-then-swap、`tokio::Mutex` 串行化、单上游失败隔离）。
  - **M1-B.2（`downstream` MCP 服务 / eager-connect / `serve`）✅ 已完成** —— `GatewayServer`（rmcp
    `ServerHandler`，暴露 3 个固定元工具）；`upstream::connect`（`connect_all` 降级启动 + env allow-list +
    握手超时）；`gateway` 重建升级为**并发摄取 + per-ingest 超时**并加 `run_rebuild_worker`（合并 list_changed
    突发）；`mcpgw serve` 把三者装配成活的 stdio 网关。
  - **M1-C（HTTP 双向传输 + 静态 API-Key 鉴权）✅ 已完成** —— 下游经 rmcp `StreamableHttpService` 暴露 3 个元工具
    （`nest_service` 进 axum，默认 `127.0.0.1:8970` `/mcp`）+ 多 key Bearer 鉴权（常量时间比较、401）；上游新增
    `UpstreamTransport::Http`（`bearer_env` 原始 token、`headers` 头名→env 内联表）复用泛型连接管线；`serve`
    并发跑 stdio + HTTP 共享 `Arc<GatewayState>`，`tokio::select!` 统一关闭，启动期 env fail-fast。
- **后续里程碑**：完整 OAuth/DCR/反向代理（M3）、运行时热吊销 API-Key（M4）、超时主动 `notifications/cancelled`
  （继续延后）见路线图。

## 向下导航

各组件的职责与接口见 **L2**：
[catalog](./L2-components/catalog.md) · [retrieval](./L2-components/retrieval.md) ·
[config](./L2-components/config.md) · [mcpgw-cli](./L2-components/mcpgw-cli.md) ·
[upstream](./L2-components/upstream.md) · [metatools](./L2-components/metatools.md) ·
[gateway](./L2-components/gateway.md) · [downstream](./L2-components/downstream.md)
