"""Part 3 (per-crate internals) lessons — placeholders for now."""
import placeholder

LESSON_09 = placeholder.build(
    "catalog 是工具目录与命名空间层：把 N 个上游的工具收进 <code>{server}__{name}</code> 命名空间，供检索与路由使用。",
    [
        "<code>ToolDef</code> / <code>Catalog</code> 数据结构与命名空间（<code>crates/catalog/src/lib.rs</code>）",
        "<code>from_json_str</code> / <code>from_tooldefs</code> 两种装载路径",
        "<code>qualified_name = \"{server}__{name}\"</code> 规则与冲突检测",
    ],
    [
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（catalog 如何被检索/路由）"),
        ("13-retrieval-bm25.html", "13 · retrieval 内核与 BM25"),
    ],
)

LESSON_10 = placeholder.build(
    "upstream 是活的上游 MCP I/O 层：rmcp client 连接、工具摄取、call_tool 转发，支持 stdio 子进程与远程 HTTP，连不上则降级隔离。",
    [
        "<code>UpstreamHandle</code> / <code>UpstreamRegistry</code>（<code>crates/upstream/src/</code>）",
        "<code>connect_all</code> eager-connect、env allow-list、握手超时",
        "stdio 子进程 vs 远程 HTTP（<code>bearer_env</code> 原始 token、<code>headers</code> 头名→env）",
    ],
    [
        ("02-architecture.html", "02 · 整体架构全景（上游在哪一层）"),
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（call_tool 路由回上游）"),
    ],
)

LESSON_11 = placeholder.build(
    "gateway 持有 ArcSwap 快照状态并做 build-then-swap 重建；metatools 在不可变快照上提供 search_tools / get_tool_details / call_tool 三个元工具。",
    [
        "<code>GatewayState</code>：<code>ArcSwap&lt;GatewaySnapshot&gt;</code>（读无锁）+ rebuild 串行化（<code>crates/gateway/src/lib.rs</code>）",
        "<code>rebuild_snapshot</code>：并发摄取 + per-ingest 超时 + 原子 swap",
        "三个元工具函数与 <code>call_tool</code> 经 catalog 路由（<code>crates/metatools/src/</code>）",
    ],
    [
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（元工具数据流）"),
        ("07-vector-strategy.html", "07 · VectorStrategy（快照里装的检索策略）"),
        ("08-wiring-config.html", "08 · 装配与配置（embedder 如何注入 GatewayState）"),
    ],
)

LESSON_12 = placeholder.build(
    "downstream 把 3 个元工具暴露为真正的 MCP 服务：rmcp ServerHandler（stdio）+ Streamable HTTP（axum，多 key 常量时间 Bearer 鉴权）。",
    [
        "<code>GatewayServer</code> 实现 rmcp <code>ServerHandler</code>，<code>list_tools</code> 恒返回 3 元工具（<code>crates/downstream/src/lib.rs</code>）",
        "HTTP：<code>StreamableHttpService</code> + <code>nest_service</code> 进 axum + Bearer 鉴权层",
        "<code>serve</code>：<code>crates/mcpgw/src/main.rs</code> 用 <code>tokio::select!</code> 并发 stdio + HTTP 并统一关闭（downstream 仅提供 <code>GatewayServer</code> 与 <code>build_router</code>）",
    ],
    [
        ("02-architecture.html", "02 · 整体架构全景（下游传输能力一览）"),
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（客户端入口）"),
    ],
)

LESSON_13 = placeholder.build(
    "retrieval 是检索内核：定义 async 的 RetrievalStrategy 抽象与自研 BM25 默认实现；向量检索（第二部分）就建在这个抽象之上。",
    [
        "<code>RetrievalStrategy</code>(async, <code>#[async_trait]</code>) 与 <code>ScoredTool</code>（<code>crates/retrieval/src/lib.rs</code>）",
        "<code>tokenize</code> + <code>Bm25Strategy</code>（k1=1.2, b=0.75）算法",
        "<code>build_strategy</code> 工厂如何选择 bm25 / vector / hybrid",
    ],
    [
        ("04-vector-overview.html", "04 · 向量检索总览（与 BM25 对比）"),
        ("07-vector-strategy.html", "07 · VectorStrategy（内置 BM25 降级）"),
        ("08-wiring-config.html", "08 · 装配与配置（build_strategy）"),
    ],
)

LESSON_14 = placeholder.build(
    "config 是配置系统：解析 [server] / [upstream] / [retrieval] 等，并做启动期校验（fail-fast）。向量检索的 [retrieval.vector] 是其一部分。",
    [
        "<code>Config</code> / <code>RetrievalConfig</code> / <code>from_toml_str</code>（<code>crates/config/src/lib.rs</code>）",
        "<code>[server]</code>（stdio/http + API-Key）与 <code>[upstream]</code>（stdio/http transport）",
        "<code>validate()</code> 启动期校验规则",
    ],
    [
        ("08-wiring-config.html", "08 · 装配与配置（[retrieval.vector] 细节）"),
        ("02-architecture.html", "02 · 整体架构全景"),
    ],
)
