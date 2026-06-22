# L3 — `dashboard` 细节

只读可视化面板（子系统 A）的进程模型、数据来源、隐私边界，以及 `MetricsSink` 桶/分位算法、
`DiscoveryRingSink` ring+writer、history 限量回放的实现细节；外加**可选**的运行时禁用写子系统
（子系统 B，`admin.rs`，经 `[dashboard].admin_token_env` 开启）的鉴权、handler 流程与隐私边界。

## 进程模型：独立任务 + 独立端口 + panic 隔离 + 优雅关停

- **独立端口、独立 task**：面板是 `mcpgw serve` 里一个**与下游 stdio/HTTP 完全分开**的 axum server，
  跑在自己的 `[dashboard].bind`（默认 `127.0.0.1:8971`）上、作为一个 `tokio::spawn` 的 task，带
  `with_graceful_shutdown(oneshot)`。它**默认只读** `Arc<GatewayState>` 与两个 sink、不参与 MCP 协议；唯一的写面是
  **可选**的运行时禁用子系统（子系统 B，下文「运行时禁用写子系统」节），它只改 `GatewayState.disabled` 集并触发一次
  rebuild，**不**改配置/重启/撤 key；未配 `admin_token_env` 时面板与历史版本一致（纯只读）。
- **localhost；读端无鉴权、写端 Bearer 鉴权**：默认绑 loopback、**读端点不挂任何 auth 层**（与下游 HTTP 的 Bearer
  鉴权不同）；仅子系统 B 的 4 个 admin 写端点经 `require_admin_token` 中间件做 Bearer 鉴权（开放只读端点
  `GET /api/disabled` 不鉴权）。装配期
  若检测到「无 auth 且绑非 loopback」会 `warn`（复用 `unauthenticated_public_bind`，把面板按「无 key」判定），
  但**不**拒绝启动——外网暴露应交给反向代理。
- **Host 头校验（抗 DNS 重绑定，非鉴权）**：面板无鉴权、仅靠绑 loopback 做访问控制，但 loopback 挡不住 DNS
  重绑定——一个远端页面把自家域名重绑到 `127.0.0.1` 后即可同源 `fetch /api/*`。故**绑 loopback 时**
  （`enforce_loopback_host = !unauthenticated_public_bind(&cfg.dashboard.bind, false)`），`build_dashboard_router`
  挂一层 `require_local_host` 中间件，把 `Host` 非 `localhost`/回环 IP 的请求一律 `403`（`host_is_local` 剥端口、
  处理 IPv6 `[::1]`、含 `@`（userinfo）的 Host 直接拒、缺/不可解析 Host 判为非本地）；**绑非 loopback**（已
  `warn` 的显式外网暴露，应自行前置反代）则**跳过**该校验。这是**抗 DNS 重绑定**、**不是鉴权**——面板仍无鉴权、仅 localhost。
- **预绑定 fail-fast**：`serve` 在 spawn 任何服务 task **之前**先 `TcpListener::bind(&cfg.dashboard.bind)`，
  bind 失败即 `Err` 中止启动（对称于 HTTP listener 预绑定），避免面板 bind 失败把已起的 HTTP task 或上游
  拆卸悬挂。
- **panic 隔离靠有界缓冲**：sink 的 `record` 契约是非阻塞、不 panic；`MetricsSink` 持一把只在短聚合/快照
  期间持有的 `Mutex`，`DiscoveryRingSink` 写满即丢弃（drop-on-full）并计数，二者都不会把面板的故障传染回
  调用热路径。面板 handler 只读快照/ring，handler panic 也只影响单个请求。
- **优雅关停顺序**（见 L4 [mcpgw-main](../L4-api/mcpgw-main.md)）：`select!` 命中关停后——
  1. 信号并有界 drain HTTP task（释放其 sink clone）；
  2. **信号并有界 drain 面板 task**（`DASHBOARD_SHUTDOWN_TIMEOUT = 3s`），使其 `AppState`（含
     `DiscoveryRingSink` 的一份 clone）尽早释放；
  3. `drop(sinks)` 触发审计 channel 断连、drain 审计 writer；
  4. `drop(discovery_sinks)` + `drop(discovery_ring)` 释放最后的 `DiscoveryRingSink` clone，使 discovery
     writer 的 channel 断连，再有界 drain 它（`AUDIT_DRAIN_TIMEOUT = 5s`）。
  把面板放在审计/discovery writer drain **之前**关停，是为了先释放它持有的 ring clone，让 writer 能干净 drain。

## 数据来源

面板的每个视图都是**装配期注入的只读句柄**上的纯函数（`api.rs`），按需 `snapshot()`：

| 视图 / 端点 | 数据来源 |
|-------------|----------|
| `/api/overview` | 活快照 `gateway.snapshot().catalog().len()`（工具数）、`MetricsSink::snapshot().total_calls`、`gateway.last_summary()`（本次重建 `skipped` 数）、`upstreams` 配置数与连接数、`uptime` |
| `/api/upstreams` | 每个配置上游：状态来自 `last_summary`（`ingested` → `connected`、`skipped` → `skipped`+原因、无 summary → `unknown`）、工具数来自 `catalog()` 按 `server` 过滤、调用/错误来自 `MetricsSink` 的 `per_upstream` |
| `/api/upstreams/{name}` | `upstream_detail`：上述 `UpstreamView` 字段 + 该上游当前工具表（`catalog()` 按 `server` 过滤）；非配置上游 → 404 |
| `/api/tools?q=` | 活快照 `catalog()`，可选子串过滤（对 `qualified_name` 与 `description` 做小写 `contains`） |
| `/api/tools/{name}` | `tool_detail`：按 qualified `{server}__{tool}` 在 `catalog().get(name)` 取（schema + 所属上游）；缺 → 404 |
| `/api/metrics` | `MetricsSink::snapshot()`（**实时**聚合） |
| `/api/traces?source=live` | `DiscoveryRingSink::recent(limit)`（内存 ring；未启用 discovery → 空） |
| `/api/traces?source=history` | `replay_discovery_items(discovery_path, TRACE_HISTORY_SCAN)`（discovery JSONL 回放；无 path → `history_unavailable`） |
| `/api/traces/{id}` | `h…` → discovery 回放定位（重扫 `TRACE_HISTORY_SCAN`）；否则 `DiscoveryRingSink::get(seq)`；找不到 → 404 |
| `/api/metrics/history` | `replay_audit_metrics(audit_path, limit, bucket_ms)`（审计 JSONL 回放；无 path → `history_unavailable`） |
| `/api/calls?source=live` | `CallRingSink::query(filter, limit, offset)`（内存环；未启用 → 空、`history_unavailable=false`；**列表省略 args/result 内容**） |
| `/api/calls?source=history` | `replay_audit_calls(audit_path, scan_limit, filter)`（审计 JSONL 回放；无 path → `history_unavailable`；**审计仅元数据，内容恒 `None`**） |
| `/api/calls/{id}` | `h…` → 审计回放定位（重扫 `CALL_HISTORY_SCAN`）；否则 `CallRingSink::get(seq)`（**详情带 args/result 内容**）；找不到 → 404 |

即面板把**五类来源**拼起来：① 活快照（catalog + last_summary）、② 实时 `MetricsSink`、③ 实时
`DiscoveryRingSink`、④ 实时 `CallRingSink`（逐条调用环）、⑤ 历史 JSONL 回放（审计 + discovery，审计同时供
逐条 `replay_audit_calls`）。**实时与历史是两条独立来源**：实时来自内存 sink，历史来自落盘 JSONL，互不依赖
（任一缺失只让对应视图降级，不影响其它）。

- 逐条调用走新增的 `CallRingSink`（内存环，`[dashboard].call_buffer` 上界，满淘汰最旧）+ 可选 audit JSONL
  历史回放（`replay_audit_calls`），与 Traces 的「实时环 + 历史回放」双源模型一致；经 `/api/calls`（列表）与
  `/api/calls/{id}`（详情）暴露。
- **M3 下钻详情（traces 也带稳定 id）**：traces 现像 calls 一样为每条分配**稳定 id**——live = ring 内单调
  `seq`（`record` 在锁内 `fetch_add` 分配，保证物理序恒等于 seq 序），history = `"h{ts}-{n}"`（同 `ts` 文件序内
  第 n 条）；list 与 detail **共用 `TRACE_HISTORY_SCAN = 50_000` 扫描窗口**，故 list 分配的 history id 在 detail 同窗
  口稳定复现（镜像 `CALL_HISTORY_SCAN`）。在此之上新增**三个详情端点** `/api/{upstreams/{name},tools/{name},
  traces/{id}}`（各 `Json<…Detail>`/`Json<TraceItem>` 或 404）与**三个详情视图**（`UpstreamDetail`/`ToolDetail`/
  `TraceDetail`），与现有列表 + `Overview` 可点卡片一起构成 **列表→详情→交叉跳转** 闭环：上游↔工具↔调用↔追踪
  （`UpstreamDetail`↔`ToolDetail`、`*Detail`→近期 `/api/calls`、`CallDetail`→工具/上游、`TraceDetail`→命中工具）。
  注：discovery JSONL writer 仍只写**原始 `DiscoveryRecord`**（不含 seq/id），id 是 API 出口侧赋予的。

## 前端与构建链（Svelte 5 + Vite，产物内嵌）

- **从原生 JS 升级为 Svelte 5 + Vite**：面板前端由原先约 55 行的零依赖 vanilla-JS 单面板，升级为 `crates/dashboard/ui/`
  下的 **Svelte 5 + Vite** 多视图 hash-路由应用（组件源在 `ui/src/`）。
- **构建链**：`npm run build`（Vite）→ `ui/dist/`（hash 命名的多文件 JS/CSS + `index.html`，**已入库**）→
  `rust-embed` 在 `assets.rs` 编译期整目录内嵌 → `cargo build --locked` **无需 node 工具链**（产物即仓库内静态文件，
  `node_modules/` gitignore、`dist/**` 在 `.gitattributes` 标记 generated）。改了前端须重跑 `npm run build` 再提交 `dist/`。
- **静态交付变化**：由原先 `include_str!` 三文件（`/`、`/app.js`、`/style.css`）改为单个 `assets::static_handler`
  挂在 router `.fallback`（`/` → 内嵌 `index.html`、`/assets/*` → 内嵌资源；未知路径回退 `index.html`）。**hash 路由**
  让 fragment 不发往服务端，故深链刷新只请求 `/`，**无需 history 回退改写**；hash params 经 `decodeURIComponent` 解码。
  `/api/*` 端点数由 **13 → 18 个**：原 13 读 + 子系统 B 新增的 **1 个开放只读** `GET /api/disabled`（共 14 读）+
  **4 个 Bearer 鉴权写** `POST /api/admin/{upstreams,tools}/{name}/{disable,enable}`（M3 详情下钻 + M1 `/api/calls`、
  `/api/activity`、`/api/about` 属原 13 读）。
- **视图**：Overview（指标卡，**可点直达** upstreams/tools/calls 列表）、Calls（指标卡 → 逐条列表 → 详情下钻：
  `/api/metrics` 可点击卡过滤 `/api/calls`，行进 `/api/calls/{id}`）、Upstreams / Tools / Traces 列表，**M3 新增三个详情视图**
  `UpstreamDetail`/`ToolDetail`/`TraceDetail`（上游/工具/追踪下钻 + 上游↔工具↔调用↔追踪交叉链接）。各视图每 3s 轮询既有
  `/api/*`。

## 隐私边界（与调用观测隔离）

- 调用观测的 `observe::CallRecord` 与审计 JSONL **仍是仅元数据**——只有 size、分类、上游名等，**永不含**
  query 文本/参数/结果内容。面板的 `overview`/`upstreams`/`metrics`/`metrics-history` 视图只读这些元数据。
- **query 文本 + 命中工具名 + 分数**走**另一条独立通道** `observe::DiscoveryRecord`（经 `DiscoverySink`），
  且**默认关闭**：仅当 `[dashboard].trace_queries = true` 时才构造 `DiscoveryRingSink` 并注入下游，下游的
  `search_tools` 分支才扇出 `DiscoveryRecord`。该通道与仅元数据的 `CallSink` 物理隔离，绝不让 query 漏进
  tracing/审计。`/api/traces` 是唯一会回显 query/工具名的端点。
- SPA 渲染对**所有不可信字段**（query、工具名、上游名、skip 原因、transport、meta_tool 名）依赖 Svelte 的 `{expr}`
  **自动 HTML 转义**，全前端**不用** `{@html}`，避免上游/客户端控制的字符串造成 stored XSS（`assets.rs` 单测扫描
  `ui/src` 锁死「无 `{@html}`」）。

## 调用内容捕获（M1，与元数据/审计隔离）

逐条调用的 **args/result 内容**（含上游错误文本）经一条**独立** `observe::CallContentSink` 通道捕获——**与
仅元数据 `CallRecord` → tracing/审计/metrics 路径完全独立、互不影响**：

- **独立通道、元数据路径不变**：下游 `call_tool` 先按原样把 `CallRecord` 扇出给 `sinks`（tracing / 审计 JSONL /
  `MetricsSink`，**这条仅元数据路径一字未改**），随后**仅当** `content_sinks` 非空时，另把一条
  `CallContent { args, args_truncated, result, result_truncated }` 扇出给 `content_sinks`。dashboard 的
  `CallRingSink` **只**注入 `content_sinks`，`MetricsSink` 仍在元数据 `sinks`——故内容**绝不**抵达 tracing/审计/指标。
  **审计 JSONL 仍仅元数据**（`replay_audit_calls` 回放出的 history `CallItem` 内容恒 `None`）。
- **只在内存、重启即丢**：内容仅活在 `CallRingSink` 的内存环里（容量 `[dashboard].call_buffer`，满淘汰最旧，
  **不落盘**），故常驻内存按 `call_buffer × 2 × payload_max_bytes` 有界（args 与 result 各自封顶 `payload_max_bytes`）、进程重启即全部丢失。
- **单条 UTF-8 截断**：下游用 `cap_json`（args）/`cap_response`（result，`Err` 走上游错误纯文本）把每条载荷各自
  截到 `[dashboard].payload_max_bytes`（默认 16384）字节，截断在 `char` 边界进行（**绝不切碎码点**），并以
  `*_truncated` 标记是否触顶。
- **详情含内容、列表不含**：`CallRingSink::get(seq)`（→ `/api/calls/{id}` 详情）以 `to_item(true)` 带出
  args/result；`query`（→ `/api/calls` 列表）以 `to_item(false)` **省略**内容（`CallItem` 的内容字段 `None` 经
  `skip_serializing_if` 不出现）——故列表轻量、内容只在按 id 下钻时回显，`CallDetail.svelte` 展示 Arguments/Result。
- SPA 同样对 args/result 走 Svelte `{expr}` 自动转义（无 `{@html}`），故捕获的载荷文本不构成 XSS。
- **内容过滤（M2，仅 live）**：`/api/calls` 支持 `q`（自由文本：对 args+result 的截断 JSON 文本做大小写不敏感子串，故可命中 JSON 键名与标点）与 `arg_key`+`arg_val`（结构化：解析 args JSON 后**递归**找任一键 `== arg_key`（精确、大小写敏感）且字符串化值 `contains` `arg_val`（大小写不敏感）；容器值会被字符串化，截断/非法 JSON → 不命中）。两参数**必须成对**给出，缺其一静默忽略。内容过滤**只对带内容的 live 项**生效：`CallFilter::matches` 把内容检查门控在 `Some(args)` 之内，history 回放无内容（`args==None`）故 `source=history` 时内容过滤被自然忽略。**性能（metadata-first）**：`query` 仅当存在内容过滤时 `want_content=true`；先用轻量 `to_item(false)` 跑元数据谓词，无内容过滤时直接走轻量路径（与 M1 同成本），**仅对元数据幸存者**才付 `to_item(true)` 构建内容并复跑过滤，最后剥离页内内容——**列表始终不含内容**。前端在主 Calls 页与 `UpstreamDetail`/`ToolDetail` 详情页的 Recent-calls 列表均提供内容搜索 UI（history 下禁用）。
- **测试覆盖**：`calls.rs` 内容过滤单测（`query_free_text_filters_over_args_and_result`、`query_arg_key_value_recurses_nested_args`、`query_free_text_matches_result_only` 等，含截断/非法 JSON 不命中的边界）+ mock-上游 e2e（`q=hi`/`arg_key=text&arg_val=hi` 命中、非匹配 `q` 返回 `total=0`）。

## 活动聚合（M3，`/api/activity`，只读、仅元数据）

`/api/activity?window=<ms>` 把 live 调用环窗内记录聚合为面板的趋势 sparkline + error_kind 分布 + 最慢/最忙 Top-5。`CallRingSink::activity(window_ms)` 在锁内把每条环记录投影为 `activity::AggInput`（**仅元数据**：`seq`/`ts`/`meta_tool`/`target_tool?`/`latency_ms`/`outcome`/`error_kind?`，**绝不读 `content` 的 args/result**），释放锁后交给纯函数 `activity::aggregate`。dashboard 未启用（`calls` 为 `None`）时 `api::activity` 退化为 `aggregate(&[], …)`，回 24 个全 0 桶。

- **窗口解析（`parse_window`）**：查询参数 `window`（ms）缺省 `900_000`（15min），`clamp(60_000, 86_400_000)` 即 [1min, 24h]；解析失败回退缺省。
- **固定 24 桶**：`bucket_ms = (window/24).max(1)`、`span = bucket_ms*24`、`start = now - span`；桶 `i` 起点 `start + i*bucket_ms`，`now` 落**末桶**（柱数恒为 24，渲染稳定）。
- **窗外不计**：`ts < start` 的记录跳过；桶索引在 u64 内 `.min(23)` clamp 后才转 usize（防远未来 ts 越界）。
- **错误口径**：`errors` = `outcome != "ok"`（`error`/`timeout` 均计错，同时累加到所在桶的 `errors`）。
- **`by_error_kind`**：仅统计带 `error_kind` 的记录（即非 ok 调用），count 降序、并列 kind 名升序。
- **`busiest_tools`**：**仅按 `target_tool`** 计数（`search_tools` 无 target，不计入），count 降序取 Top-5。
- **`slowest`**：按 `latency_ms` 降序（并列按 ts 降序）取 Top-5；`label` 取 `target_tool`，无则回退 `meta_tool`。
- **隐私边界**：`ActivityResponse`（及其 `ActivityBucket`/`KindCount`/`SlowCall`/`ToolCount`）**类型层面**就不含任何内容字段；单测 `response_has_no_payload_content_fields` 断言序列化 JSON **不含** `"args"`/`"result"`，环级 `activity_aggregates_live_ring_window` 另断言**不含** `"args"`。该端点走 live 内存读，无 `spawn_blocking`。
- **前端交互（点柱筛 Calls，纯前端、后端零改动）**：sparkline 现为**可交互 flex 柱**——非零柱顶部显示该桶调用数、
  点击以 `since`/`until` **闭区间** `[t, t+bucket_ms-1]` 把 Calls 列表筛到该桶窗。Calls 内点柱直接本页筛、Overview
  点柱经 `pendingBucket`（`bucketSel.svelte.js`）跨页带窗跳 `#/calls`（Calls 组件初始化时消费一次）。桶筛（绝对
  `since`+`until`）与滚动时间范围**互斥**（选范围 chip 清桶选、反之亦然），并有「selected bucket」chip 可一键清除。
  复用既有 `/api/calls` 的 `since`/`until` 闭区间，`/api/activity` 不变。

## About/Settings（`/api/about`，启动时组装、运行期不变、仅非敏感）

`/api/about` 回 dashboard 的 About/Settings 视图：**启动时**由 `AboutInfo::from_config(&cfg, version)` 从生效配置 + 版本组装**一次**、存进 `AppState.about`，`h_about` 直接 `clone` 序列化（**运行期不变、零计算**、无 `spawn_blocking`、无锁）。展示生效的非敏感配置/限额（retrieval 策略/top_k、dashboard 缓冲/截断、audit 开关/路径、server 监听形态、各上游 transport/超时）+ 版本/构建信息。

- **隐私边界（仅非敏感、绝不含密钥/env）**：`AboutInfo` 及其嵌套类型（`VersionInfo`/`RetrievalInfo`/`DashboardInfo`/`AuditInfo`/`ServerInfo`/`UpstreamConfigInfo`）**字段集里根本不含**任何密钥/token/env 名/env 值/上游认证引用。`server.http_auth` **仅 bool**（`!api_keys.is_empty()`，有无鉴权），**绝不**带键名或 env 名；`dashboard.admin_enabled` 同理**仅 bool**（`admin_token_env.is_some()`，镜像 `http_auth`），**绝不**带 admin env 名或 token 值；上游只暴露 `name`/`transport`/`call_timeout_ms`，**不含** url/bearer_env。单测 `http_auth_true_and_no_secrets_leak` 用含 `api_key`(env `SECRET_KEY`) + http 上游(`bearer_env=REMOTE_TOKEN`) 的配置组装后断言序列化 JSON **不含** `SECRET_KEY`/`REMOTE_TOKEN`/`keylabel`/`example.com`/`bearer_env`/`api_key`；`admin_enabled_reflects_config_without_leaking_env_name` 另用配了 `admin_token_env="MCPGW_DASH_ADMIN"` 的 dashboard 断言 `admin_enabled==true` 但 JSON **不含**该 env 名。
- **版本/构建信息**：`VersionInfo` 由 `main.rs` 注入——`version=CARGO_PKG_VERSION`、`git_sha=MCPGW_GIT_SHA`、`build_time=MCPGW_BUILD_TIME`（后两者由 `crates/mcpgw/build.rs` 在编译期写入，git/构建时间失败时优雅降级为 `"unknown"`/`0`，详见 [`mcpgw-main`](../L4-api/mcpgw-main.md)）。
- **`transport_label` 自包含**：`about.rs` 私有的 `transport_label`（`Stdio→"stdio"`/`Http→"http"`）**不复用** `mcpgw` 的 `transport_str`，使 dashboard crate **不反向依赖 mcpgw**。
- **mock-上游 e2e**（`dashboard_detail_endpoints_with_mock_upstream`）断言 `version.version` 非空、`server.http_auth=false`（测试配置无 api_key）、`upstreams` 含 `{name:"mock", transport:"stdio"}`。

## 运行时禁用写子系统（子系统 B，`admin.rs`）

面板的**唯一写面**，**默认关闭**：仅当 `[dashboard].admin_token_env` 指定了一个**持有 Bearer token 的环境变量名**
时才启用。它只做「临时 disable/enable 上游或单工具」——改 `GatewayState.disabled` 集并触发一次 rebuild，
**不**改配置文件、不重启、不撤 API-key。未配置 token 时面板与历史版本完全一致（纯只读）。

### 鉴权：`require_admin_token` 中间件（404 / 401 / pass）

admin 层经 `route_layer` **只挂在 4 个 admin POST 路由**上（开放只读端点 `GET /api/disabled` 不经过它）：

- **未配置**（`AppState.admin_token == None`）→ **404**：写端点表现得**像不存在**，不向未授权者泄漏「这里有 admin 子系统」。
- **配了但 Bearer 缺失 / 方案错 / 值不匹配** → **401**。
- **Bearer 精确匹配** → 放行。

token 比较走 `authorize()` 纯函数 + `subtle::ConstantTimeEq` **常量时间**比较（镜像下游 `http.rs` 的 key 校验，
抗计时侧信道）；`presented_bearer` 解析 `Authorization` 头（scheme 大小写不敏感、空值视为缺失）。

### handler 流程：幂等优先 → 存在性校验 → spawn_blocking 变更 → rebuild

4 个 handler（`disable_upstream`/`enable_upstream`/`disable_tool`/`enable_tool`）统一流程：

1. **幂等读检查**：先**只读** `is_*_disabled` 看是否已是目标态。已是（重复 disable/enable）→ **直接回当前
   `DisabledSnapshot`（200）、不 mutate、不 rebuild**（幂等、省一次重建）。
2. **存在性校验**（仅 disable，**在 mutate 之前**）：disable 时校验目标真实存在——上游须 ∈ 配置的上游列表、工具须
   ∈ 当前 catalog；不存在 → **404**。因校验先于变更，**绝不会把幽灵名写进禁用集**。enable 不做存在性校验（清除陈旧名合法）。
3. **`mutate_and_rebuild`**：把**同步、会 `fsync` 的持久化变更**放进 `tokio::task::spawn_blocking`（不阻塞 axum
   执行器），随后 `await rebuild_snapshot()`。**rebuild 失败 → 500**（变更已落 `DisableSet`，快照下次重建自会收敛）；
   成功 → 回更新后的 `DisabledSnapshot`（200）。
4. **记录变更**：`info!` 一条结构化日志（动作 + 目标名；**绝不**含 token）。

### 与 gateway 耦合 + in-flight 竞态

- handler 经 `gateway.disabled_arc()` 拿到与 rebuild **同一** `Arc<DisableSet>`，故 disable 写入对下一次 rebuild 立即可见。
- **隐藏式语义**：禁用项在下次 rebuild 后从快照消失，下游对其表现为 `ToolNotFound`；`metatools`/`downstream` 零改动。
- **in-flight 调用可能再完成一次**：禁用只在 rebuild 生效，若某 `call_tool` 在禁用落地前已通过 `find` 解析到工具并发起，
  它可能在禁用后**再成功返回一次**（check-then-call 竞态）；这是无锁读路径的固有取舍，**不泄漏状态**，下次 rebuild 后彻底消失。

### 持久化解耦于 `dashboard.enabled`

`DisableSet` 由 `main.rs` 用 `DisableSet::load_or_new([dashboard].disabled_state_path)` 在**首次 rebuild 之前**经
`with_disabled` 注入，且**独立于 `dashboard.enabled`**——**即使不开 dashboard**，已持久化的禁用项也从启动起生效
（持久化是 gateway 行为，非 dashboard 行为）。`disabled_state_path = None` 时纯内存、重启即清。持久化的原子写/自愈
细节见 [gateway L3](./gateway.md) 的 `DisableSet` 节。

### 开放只读端点 `GET /api/disabled`

`disabled` handler（**不**经 admin 中间件）回 `DisabledSnapshot { upstreams, tools }`（有序名列表）。前端据此渲染
「已禁用」徽标，并在被禁工具因隐藏语义从 `/api/tools` 消失时单列一个「已禁用工具」区。该端点**只读名字**、无内容、无鉴权。

### 前端：admin token 仅在浏览器内存

- admin token **只存浏览器内存**（组件状态），**绝不**写 localStorage/sessionStorage、不入 URL、不随只读请求发送——
  仅在 4 个 admin POST 的 `Authorization` 头里带上；刷新即清、需重新输入。
- 可复用的 `DisableToggle` 按钮**仅在已输入 token 时显示**；禁用徽标来自 `/api/disabled`；About 页显示 `admin_enabled`
  徽标 + 一个 token 输入框。
- **隐私**：服务端把 token 当作**对 env 名的引用**在启动期解析，**绝不**写日志、绝不进 `/api/about`（`admin_enabled`
  仅 bool）、绝不持久化；前端只在内存里持有解析值。

## `MetricsSink`：固定桶直方图 + 近似分位

- **固定桶上界（ms）**：`BUCKETS_MS = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 5000, u64::MAX]`（最后一桶无界）。
  每条记录按 `latency_ms <= bucket` 落入首个匹配桶，并更新 `max_ms`。
- **错误判定**：`is_error = !matches!(outcome, CallOutcome::Ok)`——`Error` 与 **`Timeout` 都算 error**
  （与历史回放 `replay_audit_metrics` 把非 `"ok"` 记为 error 一致，使实时/历史口径对齐）。
- **近似分位**：`percentile(p)` = 累计计数首次 `>= ceil(p * calls)` 的那个桶上界，并 `min(max_ms)` 封顶，
  故 `p50 <= p95 <= max` 单调、且**绝不超过实测最大值**。`calls == 0` 时返回 0。
- **`per_upstream` 上限**：`upstream` 名理论上可被 client 影响（虽已由下游的「解析工具目录查真实 `server`」
  安全修复收紧，见下），故 `per_upstream` 键数封顶 `MAX_UPSTREAM_KEYS = 1024`：已存在的键照常累加，新键只在
  未达上限时插入；**总调用数仍全部计入** `total_calls`（封顶只丢「按上游细分」的维度，不丢总量）。

## `DiscoveryRingSink`：环形缓冲 + 可选后台 writer

- **ring**：`VecDeque<StoredTrace>`（每项 `{ seq, record }`），容量 `cap.max(1)`；`record` 在锁内先
  `next_seq.fetch_add` 分配单调 seq 再满则 `pop_front`、`push_back`，`recent(limit)` 用 `iter().rev().take(limit)`
  给出 newest-first 的 `TraceItem`（live id = seq），`get(seq)` 取单条。读写都在短临界区内的 `Mutex`。
- **可选 writer**：`spawn(cap, Some(path))` 打开 discovery JSONL（create+append）+ 起命名 `discovery-writer`
  的 OS 线程；`record` 在写 ring 后把 `serde_json::to_string(rec)` `try_send` 进容量 `WRITER_CHANNEL_CAP = 1024`
  的有界 channel，**满则计数丢弃**（`dropped_count`），**绝不阻塞**调用热路径。
- **writer loop**：`recv` 一行即写 + 把积压 `try_recv` 排空批量写 + 每批 `flush`；干净断连（所有 sink clone
  drop）时**最终 flush + `sync_all`（fsync）**再退出。`DiscoveryWriter::join` 阻塞至此完成。
- discovery JSONL 与审计 JSONL 是**两个不同文件**（`[dashboard].trace_path` vs `[audit].path`），各自独立
  writer，互不影响。

## history 回放：尾部限量 + 优雅降级

- **尾部限量**：私有 `tail_lines(path, limit)` 用一个容量 `limit` 的 `VecDeque` 滚动读，**任意时刻内存里最多
  `limit` 行**，故对一个超大 JSONL（每行一条有界记录）也是有界内存。文件打不开返回 `None`（→ 视图
  `history_unavailable`）；读到非 UTF-8/IO 错误的行即提前结束尾扫（我们写出的 JSONL 总是合法 UTF-8，该路径
  只对外部损坏文件生效）。
- **handler 侧再封顶**：`/api/traces`、`/api/metrics/history` 的 `limit` 在 handler 里先 `min(MAX_HISTORY_LIMIT
  = 50_000)`，避免敌意/失手的巨值让 `tail_lines` 缓冲过多行。
- **坏行跳过**：`replay_discovery_items` 对每行 `serde_json::from_str` 失败即跳过（并按 `ts` 文件序赋稳定
  `"h{ts}-{n}"` id）；`replay_audit_metrics` 只解析 `{ ts_unix_ms, outcome }` 两字段，坏行跳过。
- **方向**：`replay_discovery_items` 末尾 `reverse()` 给 newest-first（与实时 ring 一致）；`replay_audit_metrics`
  按 `BTreeMap` 桶键升序，oldest-first（适合画时间序列）。桶起点 `ts - (ts % bucket_ms)`，`bucket_ms` 至少 1。

## 测试覆盖

- `metrics.rs`：聚合计数/错误/延迟、分位单调且 ≤max、`Timeout` 记为 error、`per_upstream` 封顶（总量仍全计）、
  空快照清零。
- `trace.rs`：ring 封顶 + newest-first、`recent` 限量、file writer 持久化并在 `join` 时 drain；**M3 新增**
  ring 分配单调 seq 作 live id（newest-first、`recent` 首项 id 最大）且 `get(seq)` 解析命中/未命中 `None`。
- `history.rs`：缺文件不可用、`replay_audit_metrics` 分桶计数、`Timeout` 记为 error；**M1** `replay_audit_calls`：
  newest-first、稳定 `"h{ts}-{n}"` id、`filter` 在 id 分配后应用（id 不随过滤漂移）、坏行跳过；**M3** `replay_discovery_items`：
  跳坏行、newest-first、稳定 `"h{ts}-{n}"` id（同 `ts` 文件序内第 n 条）。
- `api.rs`：`overview` 报策略/上游数、`upstreams` 重建前 `unknown`、`metrics` 反映已记录调用、缺 path 时
  history 不可用、空 catalog 的 `tools` 过滤；**M1** `call_filter_from_query`、`calls`（live/history、
  分页、`history_unavailable`）、`call_detail`（live seq 与 `h…` 回放定位、未命中→`None`）、`is_history_id`；
  **M3** `upstream_detail`（未知→`None`、已配置→返回 `UpstreamView` 字段 + 工具列表）、`tool_detail`（未知→`None`）、
  `trace_detail`（live seq 解析、未知 seq / 非数字 → `None`）。
- `calls.rs`（**M1 逐条调用层**）：`CallRingSink` 满淘汰最旧 + 单调 `seq` 作 live id、`query` 最新优先且
  `total` 计全部命中、分页 `limit`/`offset`、`get(seq)`；`CallFilter::matches` 各字段过滤与 `since`/`until`
  闭区间；**M1 内容捕获** `ring_stores_content_detail_includes_list_omits`：环存 `CallContent`，`get`（详情）带出
  args/result、`query`（列表）省略它们。**M2 内容过滤** `query_free_text_filters_over_args_and_result`/
  `query_free_text_matches_result_only`（`q` 子串扫 args+result）、`query_arg_key_value_recurses_nested_args`/
  `arg_filter_matches_numeric_value`（`arg_key`+`arg_val` 递归命中含数值）、`content_filters_skip_items_without_content`
  （无内容项不被内容过滤排除）、`arg_filter_invalid_json_does_not_match_or_panic`（非法/截断 JSON 不命中、不 panic）。
  **M3 活动聚合** `activity_aggregates_live_ring_window`：环投影为 `AggInput` 后 `aggregate`，固定 24 桶、`total`/`errors` 计数、`busiest_tools` 首项命中，且序列化**不含** `"args"`（环路径不漏内容）。
- `activity.rs`（**M3 活动聚合**）：`aggregate` 纯函数单测——固定 24 桶宽/数、`now` 落末桶且窗外不计、`errors=outcome!=ok` 计桶与总、`by_error_kind` 仅非 ok 且 count 降序、`busiest_tools` 仅 `target_tool` 且 Top-5 截断、`slowest` 按 latency 降序 Top-5、`slow` label 回退 `meta_tool`、`ActivityResponse` 序列化无 `args`/`result`（隐私）。`api.rs` `parse_window_defaults_and_clamps`：缺省 15min、低/高 clamp 到 [1min,24h]、区间内透传、不可解析回退缺省。
- `config`（**M1**）：`[dashboard].call_buffer` 默认 `2000`、`call_buffer = 0` 被 `validate` 拒绝；
  `[dashboard].payload_max_bytes` 默认 `16384`、`payload_max_bytes = 0` 被 `validate` 拒绝。
- `lib.rs` / `assets.rs`：内嵌 UI 就位且接线（`assets.rs` 测内嵌 `index.html` 含 Svelte 挂载点 `id="app"`、
  `assets/` 下有一份 hash JS、`no_svelte_component_uses_raw_html` 扫描 `ui/src` 禁 `{@html}` 防 XSS）、`host_is_local`
  接受回环/`localhost`/IPv6 `[::1]` 且拒远端域名、非回环 IP、含 `@` 的 Host 与缺失 Host。
- `crates/mcpgw/tests/dashboard.rs`（**默认 `#[ignore]`**，绑端口，`--ignored` 跑）：`serve` 起面板、
  `/api/overview` 报 `strategy=bm25`、一次 `search_tools` 被 `/api/traces?source=live` 捕获到 query；**M2** 断言
  `/` 交付内嵌 SPA（`text/html` 且含挂载点 `id="app"`）；**M3** 在同测里加 trace 详情 happy-path（`/api/traces/{id}`
  返回该 query）+ 未知上游/工具/追踪 → 404；并新增第二个 `#[ignore]` e2e `dashboard_detail_endpoints_with_mock_upstream`：
  以真实 `mock-stdio` 上游（4 工具 echo/greet/slow/fail）驱动一次 search + 一次 `call_tool`，断言 `/api/upstreams/mock`
  （`tools_count=4`、含 `mock__echo`）、`/api/tools/mock__echo`（`server=mock` + `input_schema`）、`/api/traces/{id}` 详情
  happy-path，**并（M1 内容捕获）**断言 `/api/calls?source=live&meta=call_tool` 列表项**不含** `args`，再按其 id 取
  `/api/calls/{id}` 详情**含** `args`（含回显文本 `hi`）与 `result`（mock-upstream 命中路径 e2e 默认 `#[ignore]`；
  **并（M2 内容过滤）**断言 `/api/calls?source=live&meta=call_tool&q=hi` 与 `&arg_key=text&arg_val=hi` 各命中 ≥1、
  非匹配 `q=zzz_no_match_zzz` 返回 `total=0`；
  mock-stdio 缺失时优雅跳过，**需先 `cargo build -p upstream --features testkit --bin mock-stdio` 再 `cargo test -p mcpgw --test dashboard -- --ignored`**，或设 `MCPGW_REQUIRE_MOCK=1` 让缺二进制时硬失败以确保真跑；仓库当前无 CI 跑 ignored 测试）。

## 相关

- 接口见 L2：[dashboard](../L2-components/dashboard.md)；逐文件 API 见 L4：[dashboard](../L4-api/dashboard.md)
- 接缝来源见：[observe-lib L4](../L4-api/observe-lib.md)（`DiscoverySink`/`DiscoveryRecord`）·
  [downstream L3](./downstream.md)（`search_tools` 捕获 + `upstream` 归因安全修复）
- 装配/关停顺序见：[mcpgw-main L4](../L4-api/mcpgw-main.md)；配置见：[config L3](./config.md)（`[dashboard]` 段）
