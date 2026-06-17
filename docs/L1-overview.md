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
- `retrieval` → **仅依赖 `catalog`**（不依赖 `config`/CLI，**也不引入任何 HTTP 依赖**）。`build_strategy` 故意接受
  策略名字符串（+ 打包的可选后端 `Backends`）而非配置类型，保持核心排序 crate 的独立可复用性（M2.T5 起签名为
  `build_strategy(name: &str, backends: &Backends)`，`Backends { embedder, chat, subagent_candidates }`）。
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
   │  tokio::select! over { stdio waiting() · HTTP 任务自结束 · ctrl_c } → 优雅关闭          │
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
- **进程模型**：`serve` 按配置并发跑 stdio 与/或 HTTP，共享同一 `Arc<GatewayState>`；HTTP 作为
  带 `with_graceful_shutdown`（oneshot 驱动）的后台任务，前台 `tokio::select!` 等待关闭触发
  `{stdio waiting()、ctrl_c、HTTP 自结束}`，再有界排空 HTTP→审计 drain→上游收尾（详见 L3
  [mcpgw-cli](./L3-details/mcpgw-cli.md)）；**至少须启用一种传输**。
- **Fail-fast**：所有 env 引用的密钥/头值在启动时解析校验，缺失即报错并指明字段名与 env 变量名（**绝不泄露值**）。
- **继续延后**：完整 OAuth/DCR/反向代理正确性 → M3；运行时热吊销/增删 API-Key → M4；超时主动向上游发
  `notifications/cancelled` → 仍延后（与 HTTP/鉴权正交，drop in-flight future 在 Rust 里已安全）。
- 接口/细节见 L2/L3/L4：[config](./L2-components/config.md) · [downstream](./L2-components/downstream.md) ·
  [upstream](./L2-components/upstream.md) · [downstream/http.rs](./L4-api/downstream-http.md) ·
  [upstream/connect.rs](./L4-api/upstream-connect.md)。

## M2-A 新增：异步可插拔检索 + 向量/混合/subagent 策略 + `embedder`/`chat` crate（已完成）

把检索从「**仅 BM25、同步**」升级为「**可插拔、异步（`async-trait`）、失败可透明降级**」：默认仍是 BM25，
另加一条 **Vector** 路径——用云端嵌入做余弦检索，任何嵌入失败都**透明回退到内置 BM25**。新增**带 HTTP 依赖**的
`embedder` crate 承载真实后端 `OpenAiEmbedder`，使 `retrieval` 保持无 HTTP 依赖。
M2-B 新增 **hybrid**（RRF 融合 BM25 + 向量）路径，opt-in（需 embedder）。
M2.T5 再加一条 **subagent** 路径——BM25 预筛 → 小模型重排，并新增**与 `embedder` 对称**的第二个 HTTP-依赖 crate
`chat`（`OpenAiChat`）；装配入口从 `build_embedder` 升级为 `build_backends`（按 strategy 建 embedder 和/或 chat）。
所有新策略均 **opt-in**；**默认仍为 `bm25`**。

```
   ┌──────────────────────────── mcpgw serve（装配） ────────────────────────────┐
   │  build_backends(cfg): 按 strategy 建后端，启动期 fail-fast 读 api_key_env（只报变量名、 │
   │    绝不泄露值）→ vector/hybrid: OpenAiEmbedder → CachingEmbedder（只建一次，缓存跨快照）；│
   │    subagent: OpenAiChat + subagent_candidates；bm25: 空 Backends                       │
   │  prepare_state: GatewayState::with_backends(strategy, backends)                        │
   └───────┬───────────────────────────────────┬───────────────────────────┬──────────────┘
           │ Backends.embedder 注入             │ Backends.chat 注入        │ HTTP（两个 HTTP-依赖 crate）
   ┌───────▼──────────────────────────┐  ┌─────▼──────────────────┐  ┌─────▼──────────────────────┐
   │  retrieval（无 HTTP 依赖）         │  │ embedder(reqwest+rustls)│  │ chat(reqwest+rustls)       │
   │  RetrievalStrategy（#[async_trait]）│  │ OpenAiEmbedder          │  │ OpenAiChat：POST           │
   │  build_strategy(name, &Backends)  │◄─│  POST /embeddings、按    │  │  /chat/completions、        │
   │   bm25 → Bm25Strategy（无需后端）  │  │  index 排序、dim 校验    │  │  temp=0、bearer、非2xx 截断 │
   │   vector/hybrid → 需 embedder     │  │  impl retrieval::Embedder│  │  impl retrieval::ChatModel │
   │   subagent → 需 chat              │  └─────────────────────────┘  └────────────────────────────┘
   │  Embedder/ChatModel trait · EmbedError/ChatError · CachingEmbedder（文本键记忆，仅嵌未命中）│
   └───────┬──────────────────────────────────────────────────────────────────────────────────┘
           │ VectorStrategy = 暴力余弦 + 内置 Bm25 降级；SubagentStrategy = BM25 预筛 + 小模型重排 + 降级
           ▼
       vector: index 先建 BM25 再嵌入全目录，失败→degraded；search 在 degraded/无向量/本次嵌入失败 时回退 BM25
       subagent: BM25 预筛 candidates → 小模型选工具（白名单去重）；chat/解析失败或空 shortlist → 回退 BM25
```

- **异步化**：`RetrievalStrategy` 改为 `#[async_trait]`，`index`/`search` 均为 `async`；`Bm25Strategy` 方法体
  逐字不变仅加 `async`。`GatewayState::new` **不在构造时索引**（空目录 → 首次 `rebuild_snapshot` 前 `search` 返回空）。
- **`Embedder` 抽象**（`retrieval`，`async-trait`）：`async fn embed(&[String]) -> Result<Vec<Vec<f32>>, EmbedError>`
  （保序、all-or-nothing）+ `fn dim()`；错误 `EmbedError::{Provider, Dimension{expected,got}}`（provider 无关）。
- **`ChatModel` 抽象**（`retrieval`，`async-trait`）：`async fn complete(system, user) -> Result<String, ChatError>`；
  错误 `ChatError::{Provider, Empty}`（provider 无关）；HTTP 实现 `OpenAiChat` 在 `chat` crate，`MockChatModel`
  （`testkit`）供 subagent 重排测试。
- **`CachingEmbedder`**（`retrieval`）：装饰任意 `Arc<dyn Embedder>`，以文本 `String` 为键记忆（碰撞在结构上不可能），只嵌缓存未命中、
  保序保维；在 `mcpgw` 中**只构造一次**，缓存跨快照重建持续（`list_changed` 时只嵌新增工具文本）。
- **`VectorStrategy`**（`retrieval`）：每工具嵌入文本为 `"{qualified_name}\n{description}"`，对归一化向量做暴力余弦
  （目录小，线性扫描足够），内置 `Bm25Strategy` 作**双重透明降级**（索引期嵌入失败 → `degraded`；查询期单次嵌入
  失败 → 仅本次回退），零范数守卫、不产生 NaN。
- **`SubagentStrategy`**（`retrieval`）：BM25 预筛 `candidates` 候选 → `build_user_prompt` → `chat.complete` →
  `parse_selection`（首 `[`..末 `]`、`serde_json` 解析、shortlist 白名单去重保序、剔除幻觉）→ 命中名赋合成递减分取
  `top_k`；空 shortlist 直接返回空（不调 chat），chat/解析失败或零命中**透明回退 BM25 shortlist**。prompt/parse
  纯逻辑可经 `MockChatModel` 测试。
- **`build_strategy(name, backends: &Backends)`**：`"bm25"` 无需后端；`"vector"`/`"hybrid"` 要求 `backends.embedder`
  否则 `StrategyError::EmbedderRequired`；`"subagent"` 要求 `backends.chat` 否则 `StrategyError::ChatModelRequired`；
  未知名 → `StrategyError::NotImplemented`。`Backends { embedder, chat, subagent_candidates }` 打包注入。
- **`embedder` crate**（**两个 HTTP-依赖 crate 之一**，`reqwest 0.13` + `rustls`）：`OpenAiEmbedder::new(base_url, model, api_key,
  dim: Option<usize>, timeout: Option<Duration>)` POST `{base_url}/embeddings`（bearer 鉴权），按响应 `index` 排序、校验数量/连续性/维度，非 2xx
  附**截断**的 body 片段，空输入短路返回。
- **`chat` crate**（**与 `embedder` 对称的第二个 HTTP-依赖 crate**，`reqwest 0.13` + `rustls`）：`OpenAiChat::new(base_url,
  model, api_key, timeout: Option<Duration>)` POST `{base_url}/chat/completions`（`temperature: 0`、system+user 两条消息、
  bearer 鉴权），返回 `choices[0].message.content`；非 2xx 附**截断**的 body 片段（≤500 字符），缺失/仅空白内容 → `ChatError::Empty`。
- **配置**：`[retrieval.vector]`（`base_url` 默认 OpenAI、`model`、`api_key_env`、`dim?`、`timeout_ms?`、
  `batch_size?`，`deny_unknown_fields`）；`[retrieval.subagent]`（`base_url` 默认 OpenAI、`model`、`api_key_env`、
  `timeout_ms?`、`candidates?`，`deny_unknown_fields`）；`validate()` 在 `strategy == "vector"/"hybrid"` 时要求
  `[retrieval.vector]`、`strategy == "subagent"` 时要求 `[retrieval.subagent]`（`candidates != Some(0)`）。**`batch_size` 目前为
  保留字段（未启用分块）**。密钥经 **env 变量名**引用，启动期 fail-fast 解析（缺失只报变量名，绝不泄露值）。
- 接口/细节见 L2/L3/L4：[retrieval](./L2-components/retrieval.md) · [embedder](./L2-components/embedder.md) ·
  [chat](./L2-components/chat.md) · [retrieval/embedder.rs](./L4-api/retrieval-embedder.md) ·
  [retrieval/vector.rs](./L4-api/retrieval-vector.md) · [retrieval/subagent.rs](./L4-api/retrieval-subagent.md) ·
  [embedder/lib.rs](./L4-api/embedder-openai.md) · [chat/lib.rs](./L4-api/chat-openai.md)。

## M6.T1/T3 新增 crate：`observe`（结构化调用观测 + 可选 JSONL 审计，已完成）

可观测性的第一块拼图：让**每次元工具调用**都可被结构化追踪。新增极小、无副作用的 `observe` crate，定义
**仅元数据**的 `CallRecord`、扇出契约 `CallSink` 与把记录写成结构化 `tracing` 事件的 `TracingSink`（T1）、
以及**可选的 JSONL 审计落盘** `JsonlSink`（M6.T3，std-only 专用 writer 线程）；埋点只发生在
`downstream::GatewayServer::call_tool`。**审计落盘（JSONL）已由 M6.T3 落地（默认关闭）、用量指标留待 M6.T2**。

```
        MCP 调用 ──► ┌──────────── downstream::GatewayServer::call_tool ────────────┐
                     │  start=Instant::now(); arg_bytes=len(args_json)              │
                     │  分派 metatools → (response, meta_tool, target_tool,         │
                     │                    outcome, error_kind=classify(MetaError))  │
                     │  latency_ms=start.elapsed()  ◄─ 早于结果再序列化/upstream 派生 │
                     │  result_bytes=len(resp_json); upstream=解析(target).server   │
                     │  rec = CallRecord{ ...仅 size + 分类，无任何载荷 }            │
                     └───────────────────────────┬──────────────────────────────────┘
                                                 │ for sink in sinks { sink.record(&rec) }
                          ┌──────────────────────▼───────────────────────┐
                          │  observe::CallSink（Arc<[Arc<dyn CallSink>]>）│
                          │  TracingSink ──► tracing::info!("tool_call",…)│
                          │  JsonlSink(T3,审计) · MetricsSink(子系统A)    │
                          └──────────────────────────────────────────────┘
```

- **`observe` crate**（极小、**无 HTTP/无第三方存储/不依赖兄弟 crate**，仅 `serde`/`serde_json`/`tracing`；
  审计落盘仅用 **std 文件 I/O + 专用 writer 线程**，不引入 tokio）：
  `CallRecord` 字段 = `ts_unix_ms` / `meta_tool` / `target_tool?` / `upstream?` / `latency_ms` / `outcome` /
  `error_kind?` / `arg_bytes` / `result_bytes`。**仅元数据**：`*_bytes` 是 size、**无参数/结果内容**，故观测
  绝不泄露 secret/PII（单测把序列化 key 集合**锁死**为恰好这 9 个键）。`MetaTool`/`CallOutcome` 枚举
  snake_case 序列化且 `as_str()` 与之一致（tracing 与 JSONL 用同一拼写）。
- **可选审计落盘（M6.T3）**：`JsonlSink` 实现同一 `CallSink`，`record()` 把记录序列化成一行 JSON 并 `try_send`
  进**有界 channel**（满则计数丢弃、限频 warn，**绝不阻塞调用**），专用 `audit-writer` 线程批量 drain + 每批
  flush、干净断连时**最终 fsync**；写失败只限频 warn、不退出（自愈）。`spawn_writer` 打不开文件即 fail-fast。
- **埋点只在 `downstream`**：`call_tool` 计时（延迟快照早于结果再序列化/`upstream` 派生）、经私有
  `classify(&MetaError)` 把 `Timeout/Call/ToolNotFound/UpstreamUnavailable` 映射为
  `timeout/upstream_call/tool_not_found/upstream_unavailable`、构造 `CallRecord` 再同步扇出给注入的 sink。
  **未知元工具名**是协议误用（`McpError::invalid_params`）、**不记录**。`metatools` crate 保持**纯函数、不
  依赖 `observe`**。
  - **`upstream` 归因安全修复**：`CallRecord.upstream` 现由对 `target_tool` 的**工具目录解析**取真实
    `server`（`get_tool_details(&snapshot, target).map(|def| def.server)`），**不再** `split_once("__")` 切
    client 提供的名字——未知/构造的 `call_tool` 名解析不到时 `upstream = None`，杜绝 attacker 可控前缀污染
    指标（尤见 dashboard 的 per-upstream 维度）。
- **装配**：`mcpgw serve` 构造以 `observe::TracingSink` 打底的 sinks（`Arc<[Arc<dyn observe::CallSink>]>`）并
  **同时注入** stdio（`GatewayServer::new`）与 HTTP（`build_router`）两条传输（共享同一切片），记录走 stderr
  的结构化 `tool_call` 事件；`[audit].enabled` 时追加 `JsonlSink` 落盘 JSONL，`[dashboard].enabled` 时再追加
  `dashboard::MetricsSink`，关停时有界优雅 drain（见下）。
- **扩展点**：这一「instrument → multi-sink」接缝已被 `JsonlSink`（M6.T3 审计落盘）与 `dashboard::MetricsSink`
  （子系统 A 只读面板）复用——二者都只需实现同一 `CallSink` trait、加进同一 sinks 切片即可接入，无需改埋点。
  M6.T1 另新增了**与 `CallSink` 平行的** `DiscoverySink` 契约（`DiscoveryRecord`：搜索 query + 命中工具名/分数），
  专供 dashboard 的搜索发现追踪，与仅元数据的 `CallRecord` **物理隔离、opt-in**（默认不捕获）。
- 接口/细节见 L2/L3/L4：[observe](./L2-components/observe.md) · [observe-lib](./L4-api/observe-lib.md) ·
  [observe-audit](./L4-api/observe-audit.md) · [downstream](./L2-components/downstream.md) ·
  [downstream L3](./L3-details/downstream.md)。

## 只读可视化面板（dashboard，子系统 A，已完成）

可观测性的第二块拼图：在不改动网关热路径的前提下，提供一个**只读**的本地 web 面板，把"网关现在聚合了哪些上游/
工具、调用量与延迟分位、最近的搜索发现"可视化出来。新增 `dashboard` crate，作为**独立 task、独立 port** 的
HTTP server 与网关同进程并发运行——它**只读**消费既有数据源，从不写网关状态、不参与工具路由。

```
   ┌───────────── mcpgw serve（同一进程）─────────────┐
   │  下游 stdio / HTTP（3 个元工具，热路径）          │
   │     │ 每次调用扇出 observe::CallRecord            │
   │     │ search_tools 另扇出 observe::DiscoveryRecord│
   │     ▼                                            │
   │  CallSink 切片：TracingSink · JsonlSink ·        │
   │                 dashboard::MetricsSink ◄─聚合     │
   │  DiscoverySink 切片：dashboard::DiscoveryRingSink │
   └──────────────────────┬───────────────────────────┘
                          │ 只读
   ┌──────────────────────▼───────────────── 独立 task / 独立 port ──┐
   │  dashboard server（默认 127.0.0.1:8971，localhost，无鉴权）      │
   │  AppState = { GatewayState 快照/last_summary, MetricsSink 快照,  │
   │               CallRingSink, DiscoveryRingSink, 历史 JSONL 回放 } │
   │  build_dashboard_router：8 个 /api/* + assets::static_handler   │
   │     fallback（rust-embed 内嵌的 Svelte 5 + Vite SPA）           │
   └─────────────────────────────────────────────────────────────────┘
```

- **能力**：8 个只读 JSON 端点 `/api/{overview,upstreams,tools,metrics,traces,metrics/history,calls,calls/{id}}`
  + `assets::static_handler` fallback（`/`→内嵌 `index.html`、`/assets/*`→内嵌带 hash 资源）服务一个
  **Svelte 5 + Vite 构建、rust-embed 内嵌的多视图 hash-路由 SPA**（每 3s 轮询；Svelte `{表达式}` 自动转义、
  无 `{@html}`——一个 Rust 测试扫描 `ui/src` 锁死「无 `{@html}`」——防存储型 XSS）。
- **进程模型（隔离 + 默认关闭）**：作为**独立 tokio task 起在自己的端口**上（默认 `127.0.0.1:8971`，**仅
  localhost、无鉴权**——非 loopback 绑定会 `warn!`）；由 `[dashboard].enabled` opt-in，**默认关闭**。监听端口
  在 spawn 任何 serve task **之前预绑定 fail-fast**。其聚合用**定容缓冲**（指标固定桶直方图、发现 ring buffer
  满则丢弃），故 panic/积压被隔离、**绝不反压网关热路径**。关停按固定顺序优雅 drain（HTTP → dashboard →
  审计 → 发现 writer → 上游），见 [mcpgw serve](./L4-api/mcpgw-main.md)。
- **数据源（4 个，皆只读）**：① `GatewayState` 快照（`catalog()` 工具目录）+ `last_summary()`（已摄取/被跳过的
  上游）；② `dashboard::MetricsSink`（实现 `observe::CallSink`，聚合 per-meta-tool 调用/错误/p50/p95/max +
  per-upstream，**任何非 `Ok`（含 `Timeout`）都计为错误**）；③ `dashboard::DiscoveryRingSink`（实现新的
  `observe::DiscoverySink`，最新在前的有界 ring + 可选发现 JSONL）；④ 历史 JSONL 回放（审计 + 发现两份 JSONL，
  尾部有界、优雅降级）。
- **隐私边界（沿用并强化）**：审计 JSONL 与 `observe::CallRecord` 始终**仅元数据**（size + 分类，无 query/结果
  内容）；**搜索发现追踪是一条独立、opt-in（`[dashboard].trace_queries`）的通道**——只有它带 query 文本与命中
  工具名/分数，默认关闭、与仅元数据通道**物理隔离**。
- **observe 接缝**：dashboard 不新增埋点——`MetricsSink` 复用既有 `CallSink` 多 sink 接缝；为搜索发现新增了
  **平行的** `DiscoverySink` 契约（`DiscoveryRecord` 现 `Serialize + Deserialize` 以支持 JSONL 历史回放）。
- 接口/细节见 L2/L3/L4：[dashboard L2](./L2-components/dashboard.md) · [dashboard L3](./L3-details/dashboard.md) ·
  [dashboard L4](./L4-api/dashboard.md) · 配置见 [config L3](./L3-details/config.md) 的 `[dashboard]` 段。

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
search 子命令：build_strategy(cfg.strategy, &Backends::default()) ──► strat.index(&catalog).await ──► strat.search(query, top_k).await ──► JSON
get-details 子命令：catalog.get(qualified_name) ──► 该工具完整 JSON
```

> 在最终形态（M1）里，这套"检索→详情→执行"会通过 `search_tools` / `get_tool_details` / `call_tool`
> 三个 MCP 元工具暴露给客户端；M0 先用 CLI 验证检索内核。

## 构建与测试

```bash
cargo build                 # 构建工作区（产出 target/debug/mcpgw）
cargo test --all-features   # 全部测试（253 passed / 4 ignored：catalog 4 / config 42 /
                            #   retrieval 22 + caching 4 + embedder 3 + golden 1 + hybrid 6 + subagent 7 + vector 6 /
                            #   embedder(openai) 5 + chat(openai) 4 / mcpgw main 13 + cli 5 + audit 1 /
                            #   upstream 15 + 集成 11 + http_connect 1 /
                            #   metatools 4 + call_tool 4 / gateway 8 + rebuild 8 /
                            #   observe 10 + capture 1 /
                            #   downstream 10 + e2e(stdio) 10 + e2e(http) 5 /
                            #   dashboard 43 ·
                            #   4 ignored = 门控真实冒烟（stdio + http + vector）+ dashboard e2e）
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
- **M2-A（异步可插拔检索 + 向量策略）✅ 已完成** —— `RetrievalStrategy` 改为 `#[async_trait]`（`index`/`search`
  异步）；新增 `Embedder` trait + `EmbedError` + `CachingEmbedder`（按文本键记忆，跨重建复用）；`VectorStrategy`
  在云端嵌入上做暴力余弦、内置 BM25 双重透明降级；`build_strategy`（`"vector"`/`"hybrid"` 要求 embedder）；
  新增带 HTTP 依赖的 `embedder` crate 承载 `OpenAiEmbedder`；配置 `[retrieval.vector]`
  + 启动期装配（`build_backends` fail-fast、`GatewayState::with_backends`）。**默认策略仍是 `bm25`**。
- **M2-B（混合检索 RRF）✅ 已完成** —— 新增 `HybridStrategy`：用 Reciprocal Rank Fusion（`k=60` 固定）融合 `Bm25Strategy` 词法排名与 `VectorStrategy` 语义排名（两份**全深度**子排名）；`build_strategy("hybrid", …)` 需 embedder（否则 `EmbedderRequired`），`config`/`build_backends` 将 `[retrieval.vector]` 要求扩到 hybrid；embedding 失败时经 `VectorStrategy` 内置降级自愈≈纯 BM25。**opt-in；默认仍是 `bm25`**。
- **M2.T5（subagent 重排策略）✅ 已完成** —— 新增 `SubagentStrategy`：BM25 预筛 `candidates` 候选 → 小模型（Haiku/Flash/gpt-4o-mini）重排（retrieve-then-rerank），prompt 构造/响应解析（白名单去重保序、剔除幻觉）为纯逻辑、可经 `MockChatModel` 测试；空 shortlist 不调 chat、chat/解析失败透明降级 BM25；新增 provider 无关的 `ChatModel` trait + `ChatError` 与**与 `embedder` 对称**的第二个 HTTP-依赖 crate `chat`（`OpenAiChat`，POST `/chat/completions`、`temperature: 0`、bearer）；`build_strategy(name, &Backends)` 四臂（`"subagent"` 缺 chat → `ChatModelRequired`），装配入口 `build_embedder` → `build_backends`（按 strategy 建 embedder 和/或 chat）、`GatewayState::with_backends`；配置 `[retrieval.subagent]`。**opt-in；默认仍是 `bm25`**。
- **M6.T1（结构化调用日志 + 追踪）✅ 已完成** —— 新增极小、无副作用的 `observe` crate：**仅元数据**的 `CallRecord`（只 size、无参数/结果内容，单测锁死 9 键集合）、`CallSink` 扇出契约、`TracingSink`（结构化 `tool_call` tracing 事件）；埋点**只在** `downstream::GatewayServer::call_tool`（计时—延迟快照早于结果再序列化/`upstream` 派生、私有 `classify(&MetaError)` 给 `error_kind`、构造记录并同步扇出），**未知元工具名不记录**，`metatools` 保持纯函数不依赖 `observe`；`mcpgw serve` 装配默认 `[TracingSink]` 注入 stdio + HTTP（共享同一切片）。**这一多 sink 接缝已被 M6.T3 审计 `JsonlSink` 与子系统 A 的 `dashboard::MetricsSink`（内存聚合）复用（皆实现同一 trait）；持久化用量指标（M6.T2）仍待办；与检索无关，默认策略仍是 `bm25`**。
- **M6.T3（审计持久化 / JSONL 落盘）✅ 已完成** —— 在 `observe` crate 上加**可选的 JSONL 审计 sink**：`JsonlSink`（实现同一 `CallSink`，`record()` = 序列化一行 JSON + `try_send` 进**有界 channel**，满/断连则计数丢弃 + **首次及每个 2 的幂次**限频 warn，**绝不阻塞调用热路径**；`#[derive(Clone)]`、clone 共享同一 sender + 计数）、专用 `audit-writer` OS 线程（批量 drain + 每批 flush，干净断连时**最终 flush + `sync_all` fsync** 再退出；写失败**只限频 warn、不退出**故瞬时故障自愈）、`AuditWriter`（持线程 `JoinHandle`、`join` 阻塞至 drain 完成；**不持 sender**故由所有 `JsonlSink` clone drop 触发断连）、`spawn_writer(path, capacity) -> io::Result<...>`（create+append；打不开/起不来线程即 `Err`）与 `AUDIT_CHANNEL_CAPACITY = 1024`；**仍 std-only（无 tokio）**、只用 `std::thread` + `std::sync::mpsc` + `std::fs`，`observe` 不新增依赖。配置新增 `[audit]` 段（`enabled` 默认 `false`、`path` 默认 `"mcpgw-audit.jsonl"`，`deny_unknown_fields`）；`mcpgw serve` 在 `cfg.audit.enabled` 时 `spawn_writer` **fail-fast** 追加 `JsonlSink`、`select!` 后 `drop(sinks)` 触发断连并 `timeout(AUDIT_DRAIN_TIMEOUT=5s, spawn_blocking(writer.join()))` **有界优雅 drain**（超时仅 warn）。**只落仅元数据记录（永不含 payload）；审计默认关闭、默认策略仍 `bm25`**。
- **子系统 A（只读可视化 dashboard）✅ 已完成** —— 新增 `dashboard` crate：`MetricsSink`（实现 `observe::CallSink`，聚合 per-meta-tool 调用/错误/p50/p95/max + per-upstream，非 `Ok`（含 `Timeout`）皆计错误，`per_upstream` 上限 `MAX_UPSTREAM_KEYS=1024`）、`DiscoveryRingSink`（实现新 `observe::DiscoverySink`，最新在前的有界 ring + 可选发现 JSONL writer、满则丢弃）、`history.rs`（审计/发现 JSONL 尾部有界回放、优雅降级）、`AppState` + `build_dashboard_router`（6 个 `/api/*` + 3 个静态路由 + 零构建 vanilla-JS SPA，**对所有不可信字段 HTML 转义**防存储型 XSS）。`observe` 新增 `DiscoveryRecord`/`DiscoveryHit`/`DiscoverySink` 发现追踪契约（与仅元数据的 `CallRecord` 隔离、`DiscoveryRecord` 现 `Serialize + Deserialize`）；`metatools` 的 `ToolSummary` 加 `score: f32` 并新增 `GatewaySnapshot::catalog()` 只读访问器；`gateway` 加 `GatewayState::last_summary()`（lock-free `ArcSwapOption`）；`downstream` 的 `search_tools` 在附着时扇出 `DiscoveryRecord`，并把 `CallRecord.upstream` 改为**工具目录解析**真实 server（安全修复）。配置新增 `[dashboard]` 段；`mcpgw serve` 在 `[dashboard].enabled` 时把面板起为**独立 task、独立 port**（默认 `127.0.0.1:8971`、**localhost、无鉴权**——非 loopback 绑定 warn，监听**预绑定 fail-fast**），关停按固定顺序有界 drain。**默认关闭、只读、不改网关热路径；默认策略仍 `bm25`**。
- **子系统 A · M1（逐条调用数据层）✅ 已完成** —— `CallRingSink` 内存环（`[dashboard].call_buffer` 上界、满淘汰最旧）+ audit JSONL 历史回放（`replay_audit_calls`，最新优先、稳定 `"h{ts}-{n}"` id），支撑 dashboard **Calls 下钻**；新增 `/api/calls`（列表）与 `/api/calls/{id}`（详情），使只读 API 增至 **8 端点**。**随 dashboard 启用、仅元数据、不改网关热路径；默认策略仍 `bm25`。**
- **子系统 A · M2（前端应用骨架）✅** —— vanilla-JS 扁平面板升级为 Svelte 5 + Vite 多视图 hash-路由应用（rust-embed 内嵌 ui/dist/、dist 入库故 cargo build 不依赖 node）；接通 Overview + Calls 下钻（指标卡→逐条列表→详情）并移植 Upstreams/Tools/Traces 基础视图（无回归）；静态交付由 include_str! 三文件改为 assets::static_handler fallback。
- **后续里程碑**：完整 OAuth/DCR/反向代理（M3）、运行时热吊销 API-Key（M4）、超时主动
  `notifications/cancelled`（继续延后）见路线图。

## 向下导航

各组件的职责与接口见 **L2**：
[catalog](./L2-components/catalog.md) · [retrieval](./L2-components/retrieval.md) ·
[embedder](./L2-components/embedder.md) · [chat](./L2-components/chat.md) · [config](./L2-components/config.md) ·
[mcpgw-cli](./L2-components/mcpgw-cli.md) · [upstream](./L2-components/upstream.md) ·
[metatools](./L2-components/metatools.md) · [gateway](./L2-components/gateway.md) ·
[downstream](./L2-components/downstream.md) · [observe](./L2-components/observe.md) ·
[dashboard](./L2-components/dashboard.md)
