# L2 — `dashboard` 组件

## 职责

网关的**默认只读可视化面板**（子系统 A）：把 `gateway` 的活快照、`observe` 的调用观测与可选的历史 JSONL
回放聚合起来，经一个**独立 localhost 端口**上的小 axum server 暴露为 20 个 `/api/*` JSON 端点 + 一个
**Svelte 5 + Vite 构建、经 `rust-embed` 内嵌**的 SPA（`assets::static_handler` fallback 交付）。它**默认只读**，
默认关闭、须显式 opt-in；配置 `[dashboard].admin_token_env` 后额外提供**两个 Bearer 鉴权写子系统**：**子系统 B**
（运行时禁用——仅 disable/enable 上游/工具；`GET /api/disabled` 开放只读）与 **子系统 C**（在线改配 + 上游热重载——
`GET/PUT /api/admin/config` 编辑整份 `mcpgw.toml`，`[[upstream]]` 增/删/改热重载、其余字段需重启）。二者**都不**重启
进程、不撤 key——**不配 token 时与今天完全一致的纯只读面板**。

本 crate 提供三个接入 `observe` 接缝的 sink：
- `MetricsSink` 实现 `observe::CallSink`，**实时聚合**每个元工具的调用数/错误数/延迟分位（p50/p95/max）
  与每个上游的调用/错误数；
- `CallRingSink` 实现 `observe::CallContentSink`，把**逐条** `CallRecord`（元数据）**与** `CallContent`
  （args/result 内容）一并存进**有界环形缓冲**（newest-first，上界 `[dashboard].call_buffer`），支撑 Calls 下钻的
  列表（省略内容）/详情（带内容）；
- `DiscoveryRingSink` 实现 `observe::DiscoverySink`，把 `search_tools` 的 `query → 命中工具+分数`
  追踪存进**有界环形缓冲**（newest-first），并可选地经后台 writer 线程落一份 discovery JSONL 供历史回放。

**隐私边界**：调用观测的 `CallRecord`/审计 JSONL 仍是**仅元数据**（无 query/参数/结果内容），面板的
**指标**视图只读这些元数据；**query 文本 + 命中工具名**是**另一条独立、opt-in（`[dashboard].trace_queries`）
的 discovery 通道**，绝不混入仅元数据的调用 sink；**逐条调用的 args/result 内容**则走**第三条独立通道**
`CallContentSink`，只入 `CallRingSink` 内存环（按 `[dashboard].payload_max_bytes` 单条 UTF-8 截断、重启即丢），
同样绝不混入仅元数据的 `CallSink`/审计。

## 公开接口

### 指标聚合 `MetricsSink` / `MetricsSnapshot` / `MetaToolMetrics` / `UpstreamMetrics`（`metrics.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `MetricsSink` | impl `observe::CallSink` | 内存聚合器：`record(&CallRecord)` 把一次调用计入对应元工具的固定桶延迟直方图 + 每上游计数。任何**非 `Ok`**（含 `Timeout`）记为一次 error。`per_upstream` 键数封顶 `MAX_UPSTREAM_KEYS = 1024`（防 client 控制的 upstream 名灌爆） |
| `MetricsSink::new` / `Default` | `() -> Self` | 空聚合器 |
| `MetricsSink::snapshot` | `(&self) -> MetricsSnapshot` | 拷一份当前聚合（`total_calls` + 每元工具 `MetaToolMetrics` + 每上游 `UpstreamMetrics`） |
| `MetricsSnapshot` | `Serialize` | 面板 `/api/metrics` 的响应体 |
| `MetaToolMetrics` | `Serialize` | `meta_tool` / `calls` / `errors` / `p50_ms` / `p95_ms` / `max_ms` |
| `UpstreamMetrics` | `Serialize` | `upstream` / `calls` / `errors` |

### 发现追踪 `DiscoveryRingSink` / `DiscoveryWriter`（`trace.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `DiscoveryRingSink` | impl `observe::DiscoverySink` | 最近 discovery 追踪的有界环形缓冲（容量 `cap.max(1)`，读 newest-first）；内部存 `StoredTrace{seq,record}`，有 path 时再经后台线程把每条**原始 `DiscoveryRecord`** append 成一行 discovery JSONL |
| `DiscoveryRingSink::spawn` | `(cap: usize, path: Option<&Path>) -> io::Result<(Self, Option<DiscoveryWriter>)>` | 建 ring；`Some(path)` 时打开文件（create+append）+ 起命名 `discovery-writer` 的 OS 线程，返回其 `DiscoveryWriter`（关停 drain 用）。打不开/起不来即 `Err` |
| `DiscoveryRingSink::recent` | `(&self, limit: usize) -> Vec<TraceItem>` | newest-first 最多 `limit` 条，每条带 live id = ring seq |
| `DiscoveryRingSink::get` | `(&self, seq: u64) -> Option<TraceItem>` | 按 live id（十进制 seq）取单条追踪（镜像 `CallRingSink::get`） |
| `DiscoveryRingSink::dropped_count` | `(&self) -> u64` | 因 writer channel 满而丢弃的条数（测试/诊断） |
| `DiscoveryWriter::join` | `(self)` | 阻塞至 writer 线程 drain+flush+fsync 退出（须先 drop 所有 sink clone 关闭 channel） |
| `TraceItem` | `Serialize` | 一条追踪经 API 暴露的 owned 形态（镜像 `CallItem`）：`id`（live=ring seq；history=`"h{ts}-{n}"`）/ `ts_unix_ms` / `query` / `top_k` / `results` / `latency_ms` |

### 逐条调用环 `CallItem` / `CallFilter` / `CallRingSink`（`calls.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `CallRingSink` | impl `observe::CallContentSink` | 逐条 `StoredCall{seq, record: CallRecord, content: CallContent}` 的有界环（满淘汰最旧），每条插入在锁内分配单调 `seq` 作 live id；`record(&self, meta, content)` 存元数据+内容；容量 `[dashboard].call_buffer` |
| `CallRingSink::new` | `(cap: usize) -> Self`（`cap.max(1)`） | 空环 |
| `CallRingSink::query` | `(&CallFilter, limit, offset) -> (Vec<CallItem>, usize)` | newest-first 一页 + `total`（计全部命中，独立于分页）。无内容过滤时走轻量 `to_item(false)`；存在内容过滤（`q` 或 `arg_key`+`arg_val`）时仅对**通过元数据谓词的幸存者**构造内容做过滤，随后从返回页**剥离**内容——故响应**始终省略 args/result** |
| `CallRingSink::get` | `(&self, seq: u64) -> Option<CallItem>` | 按 live seq 取单条（`to_item(true)`，**带 args/result 内容**） |
| `CallRingSink::activity` | `(&self, window_ms: u64) -> crate::activity::ActivityResponse` | 把 live 环聚合为活动洞察（**仅元数据**）：在锁内把每条 `record` 投影为 `AggInput{id=seq,ts_unix_ms,meta_tool,target_tool?,latency_ms,outcome,error_kind?}`，交纯函数 `crate::activity::aggregate(inputs, window_ms, now)` 统一按窗过滤+分桶——**绝不**触碰 args/result |
| `CallItem` | `Serialize` | live 环与 history 回放共用的 owned 项：`id` / `ts_unix_ms` / `meta_tool` / `target_tool?` / `upstream?` / `latency_ms` / `outcome` / `error_kind?` / `arg_bytes` / `result_bytes`，外加可选内容 `args?` / `args_truncated` / `result?` / `result_truncated`（仅详情填充，列表与 history 回放省略） |
| `CallFilter` | `Default` | 元数据 `meta_tool`/`upstream`/`target_tool`/`outcome`/`since_ms`/`until_ms`，外加内容过滤 `q`（自由文本，args+result 子串，大小写不敏感）、`arg_key`+`arg_val`（结构化，递归找 args 里 key=value，二者须同时给）（均 `Option`，`None`=全匹配；时间为闭区间）。内容过滤**仅对 live**（history 项无内容，`matches` 的 `Some(args)` 门控自然忽略）；`matches(&CallItem)` 统一过滤 live 与 history |

### 活动聚合 `aggregate` / `ActivityResponse`（`activity.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `aggregate` | `(inputs: &[AggInput], window_ms: u64, now: u64) -> ActivityResponse` | 纯函数：桶宽 `bucket_ms = (window_ms / 24).max(1)`，窗起点对齐到整 `BUCKETS=24` 桶；仅计 `ts_unix_ms` 落在 `[now - bucket_ms*24, now]` 窗内的 `AggInput`。统计趋势桶、错误数、`error_kind` 分布、最慢/最忙 `TOP_N=5` 双榜——**只读元数据** |
| `AggInput` | — | 聚合输入：`id` / `ts_unix_ms` / `meta_tool` / `target_tool?` / `latency_ms` / `outcome`(`"ok"\|"error"\|"timeout"`) / `error_kind?`（**无** args/result） |
| `ActivityResponse` | `Serialize` | 活动洞察响应体：`window_ms` / `bucket_ms` / `buckets[24]`（每桶 `ActivityBucket{t, total, errors}`，oldest-first，桶起点 `t` 为 unix ms）/ `total` / `errors` / `by_error_kind[KindCount{kind, count}]` / `slowest[SlowCall{id, label, meta_tool, latency_ms, outcome}]`（latency 降序 Top5）/ `busiest_tools[ToolCount{name, count}]`（计数降序 Top5）——**仅元数据** |

### 历史回放 `replay_audit_metrics` / `replay_discovery_items` / `replay_audit_calls` / `MetricBucket`（`history.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `replay_discovery_items` | `(path: &Path, limit: usize) -> (Vec<TraceItem>, bool)` | 回放 discovery JSONL 为 `TraceItem`，扫**最后** `limit` 行、newest-first，坏行跳过，稳定 id `"h{ts}-{n}"`；`bool` = 文件可读 |
| `replay_audit_metrics` | `(path: &Path, limit: usize, bucket_ms: u64) -> (Vec<MetricBucket>, bool)` | 回放审计 JSONL 进定宽时间桶（oldest-first），非 `"ok"` outcome 记为 error（与实时 `MetricsSink` 一致） |
| `replay_audit_calls` | `(path: &Path, scan_limit: usize, filter: &CallFilter) -> (Vec<CallItem>, bool)` | 回放审计 JSONL 为逐条 `CallItem`，newest-first，稳定 id `"h{ts}-{n}"`，`filter` 在 id 分配后应用；`bool` = 文件可读 |
| `MetricBucket` | `Serialize` | `bucket_start_ms` / `calls` / `errors` |

### About/Settings 视图 `AboutInfo` / `AboutInfo::from_config`（`about.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `AboutInfo` | `Serialize` + `Clone` | 启动时从 `config::Config` + 版本组装的**非敏感**生效配置只读快照（运行期不可变）：`version`（`VersionInfo`）/ `retrieval`（`strategy`/`top_k`）/ `dashboard`（`call_buffer`/`payload_max_bytes`/`trace_queries`/`trace_buffer`/`trace_path`/`admin_enabled`）/ `audit`（`enabled`/`path`，仅 `enabled` 时给 `path`）/ `server`（`stdio`/`http_enabled`/`http_bind`/`http_path`/`http_auth`——`http_auth` 是**裸 bool**=是否配了 ≥1 个 API-Key）/ `upstreams`（`Vec<UpstreamConfigInfo>`）。**字段集里根本不含**任何密钥/token/env 名/env 值（`admin_enabled` 同 `http_auth`，仅存在性 bool） |
| `AboutInfo::from_config` | `(cfg: &Config, version: VersionInfo) -> AboutInfo` | 纯映射：从生效配置摘出上述非敏感字段（HTTP 段缺省/未启用时 `http_bind`/`http_path` 为 `None`、`http_auth=false`；`http_auth = !api_keys.is_empty()`——只判存在性，**绝不**读 env 名/值）；`version` 由 `main.rs` 经 `build.rs` 注入的 `MCPGW_GIT_SHA`/`MCPGW_BUILD_TIME` 构造后传入 |
| `VersionInfo` | `Serialize` + `Clone` | `version` / `git_sha` / `build_time` |
| `UpstreamConfigInfo` | `Serialize` + `Clone` | 一个配置上游的非敏感设置：`name` / `transport`（`stdio`/`http` 短标签）/ `call_timeout_ms`（**无** url/bearer/任何认证引用） |

### API 状态与路由 `AppState` / `UpstreamInfo` / `build_dashboard_router`（`api.rs` / `lib.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `AppState` | `Clone` | 面板 handler 的只读共享态：`gateway` / `metrics` / 可选 `discovery` ring / 可选 `calls`（逐条调用环，仅 dashboard 启用时 `Some`）/ `upstreams: Vec<UpstreamInfo>` / `strategy` / 可选 `audit_path` / `discovery_path` / `started_at` / 启动时组装、运行期不可变的 `about: AboutInfo` / `admin_token: Option<Arc<str>>`（子系统 B：admin Bearer token，启动期由 `admin_token_env` 解析；`None` → 全部 `/api/admin/*` 经中间件返 404）/ **（子系统 C，6 字段）** `config_path: Option<PathBuf>`（`serve --config X` 时 `Some`，否则 config 端点 404）、`config_validator: Arc<dyn Fn(&str)->Result<Config,String>+Send+Sync>`（`main.rs` 注入的严格校验器：结构 + 全 env 引用，**避免 dashboard→main 依赖**）、`config_write_lock: Arc<tokio::Mutex<()>>`（串行化整个 PUT）、`boot_config: Arc<Config>`（启动快照，`needs_restart` 非上游基线）、`applied_upstreams: Arc<Mutex<Vec<UpstreamConfig>>>`（上游 reconcile 的"旧"基线，PUT 成功后更新、排除连接失败者）、`rebuild_trigger: mpsc::Sender<String>`（传给 `connect_all`） |
| `UpstreamInfo` | `Serialize` | 一个配置上游的静态身份：`name` / `transport`（装配期由 `Config` 给出） |
| `build_dashboard_router` | `(state: Arc<AppState>, enforce_loopback_host: bool) -> axum::Router` | 装配 20 个 `/api/*` 路由（14 读 + 6 admin 写）+ `assets::static_handler` fallback（内嵌 SPA：`/` → `index.html`、`/assets/*` → 内嵌资源），`with_state(state)`；**6 个 `/api/admin/*` 路由经 `route_layer` 单独挂 `require_admin_token` 中间件**（子系统 B 的 4 个 disable/enable POST + 子系统 C 的 `GET/PUT /api/admin/config`；开放读端点不鉴权）；`enforce_loopback_host` 时挂反 DNS-rebinding 的 Host 校验层 |

### 运行时禁用写子系统 `require_admin_token` / `disable_*` / `enable_*` / `disabled`（`admin.rs` / `api.rs`，子系统 B）

| 项 | 签名 | 说明 |
|----|------|------|
| `require_admin_token` | `async (State<Arc<AppState>>, Request, Next) -> Response` | `/api/admin/*` 的 Bearer 鉴权中间件：`AppState.admin_token` 为 `None` → **404**（未配置，不泄露存在性）；配了但缺/错 Bearer → **401**；匹配 → 放行。常量时间 `subtle::ConstantTimeEq` 比较（镜像 `downstream/http.rs` 的 api-key 路径），Bearer scheme 大小写不敏感、空 token 视为未提供 |
| `disable_upstream` / `enable_upstream` | `async (State<Arc<AppState>>, Path<String>) -> Response` | 禁用/启用一个上游 namespace：**幂等优先**（已是目标态 → 直接回 `200 Json(DisabledSnapshot)`、跳过校验与 rebuild）；disable 在真正新变更时校验 `name ∈ 配置上游`（否则 404）；改集经 `spawn_blocking` 同步持久化 → `await rebuild_snapshot`（失败回 **500**）→ 回整集 |
| `disable_tool` / `enable_tool` | `async (State<Arc<AppState>>, Path<String>) -> Response` | 同上，针对单个 qualified 工具名；disable 校验 `name ∈ 当前 catalog`（否则 404）。enable 仅移除、幂等、不校验存在性 |
| `disabled` | `(&AppState) -> gateway::DisabledSnapshot` | `api.rs` 纯函数（`h_disabled` 调用）：读 `gateway.disabled().snapshot()`，是**开放只读** `GET /api/disabled` 的响应体（永远 200；空集即 `{upstreams:[],tools:[]}`） |

> 隐藏式语义经现有代码路径达成（被禁用项经 `gateway::DisableSet` + `rebuild_snapshot` 过滤后从快照消失，
> `metatools`/`downstream` 零改动），详见 [dashboard L3](../L3-details/dashboard.md) 与 [gateway L2/L3](./gateway.md)。

### 在线改配写子系统 `get_config` / `put_config` / `atomic_write` / `restart_diff` / `ApplyResult` / `ConfigView`（`admin_config.rs`，子系统 C）

| 项 | 签名 | 说明 |
|----|------|------|
| `ConfigView` | `Serialize` | `GET /api/admin/config` 的响应体：`path: String` + `content: String`（当前 `mcpgw.toml` 原文本；文件无明文密钥，原样返回） |
| `ApplyResult` | `Serialize` | `PUT /api/admin/config` 的响应体：`upstreams: gateway::ReconcileSummary`（上游热重载结果）+ `needs_restart: Vec<&'static str>`（落盘但需重启才生效的非上游段名） |
| `get_config` | `async (State<Arc<AppState>>) -> Response` | `config_path==None` → **404**；否则 `read_to_string` 当前文件回 `Json<ConfigView>`（读失败 500）。Bearer 鉴权（经 `route_layer`） |
| `put_config` | `async (State<Arc<AppState>>, body: String) -> Response` | `config_path==None`→**404**（取锁前短路）；其余持 `config_write_lock` 串行：**空白 body**→400；`config_validator(body)` 失败→**400 + 错误消息、不写盘**；否则经 `spawn_blocking(atomic_write)` 原子落盘（失败 500）→ `gateway.reconcile_upstreams(applied_upstreams, new.upstreams, rebuild_trigger)` 热重载 → 更新 `applied_upstreams`（**排除连接失败者**，故再 PUT 重试）→ `restart_diff(boot_config, new)` → `200 Json<ApplyResult>` |
| `atomic_write` | `(path: &Path, content: &str) -> io::Result<()>`（私有） | best-effort 原子写：当前文件先复制为 `<path>.bak`（失败仅 `warn`）→ 写同目录 temp（`.tmp.{pid}`）→ `write_all` + `sync_all`(fsync) → `rename` 覆盖；**出错清理 temp**，不留半截 |
| `restart_diff` | `(boot: &Config, new: &Config) -> Vec<&'static str>`（私有） | 解构 `new` 逐段比对**非上游段**（`retrieval`/`server`/`audit`/`dashboard`）与启动基线 `boot_config`，差异段名入 `needs_restart`（解构使新增顶层段时此处编译报错，防漏） |

> 校验逻辑**注入**而非内联——`config_validator` 由 `main.rs` 的 `validate_config_text` 提供（复用启动期 env 解析器），dashboard 不反向依赖 main、`config` crate 保持纯解析。**热 vs 需重启**：仅 `[[upstream]]` 增/删/改经 `reconcile_upstreams` 秒级热重载，其余字段落盘但回 `needs_restart` 横幅（**不**重启进程）。详见 [dashboard L3](../L3-details/dashboard.md) 的「在线改配子系统」与 [gateway L2/L3](./gateway.md) 的 `reconcile_upstreams`。

`/api/*` 端点（逐符号见 L4）：`/api/overview`、`/api/upstreams`、`/api/upstreams/{name}`、`/api/tools?q=`、
`/api/tools/{name}`、`/api/metrics`、`/api/traces?source=live|history&limit=`、`/api/traces/{id}`、
`/api/metrics/history?limit=&bucket_ms=`、
`/api/calls?source=live|history&meta=&upstream=&tool=&outcome=&since=&until=&q=&arg_key=&arg_val=&limit=&offset=`（`q`/`arg_key`/`arg_val` 为内容过滤，仅 live）、`/api/calls/{id}`、
`/api/activity?window=<ms>`（活动聚合，`window` 缺省 15min、clamp 1min–24h，返回 `ActivityResponse`）、
`/api/about`（启动时组装的**非敏感**生效配置/限额 + 版本/git SHA/构建时间，运行期不可变，返回 `Json<AboutInfo>`——**绝不**含密钥/token/env 名/值）、
`/api/disabled`（**开放只读**，返回 `Json<DisabledSnapshot>`；子系统 B）、
`POST /api/admin/{upstreams,tools}/{name}/{disable,enable}`（**4 个 Bearer 鉴权写端点**，未配 `admin_token_env` → 404；逐符号见上「运行时禁用写子系统」）、
`GET/PUT /api/admin/config`（**2 个 Bearer 鉴权端点**，子系统 C 在线改配；未配 `admin_token_env` → 404；逐符号见上「在线改配写子系统」）
（M3 新增三个详情：`/api/upstreams/{name}`、`/api/tools/{name}`、`/api/traces/{id}`，各 `Json<…Detail>`/`Json<TraceItem>` 或 404；**子系统 B 新增开放 `/api/disabled` + 4 个 admin POST 使端点 13 → 18；子系统 C 再加 `GET/PUT /api/admin/config` 使 18 → 20**）。

## 依赖

- 内部：`gateway`（`GatewayState`：读活快照 + `last_summary`；**子系统 B** 另用 `DisableSet`/`DisabledSnapshot`
  与 `disabled()`/`disabled_arc()` 访问器读改运行时禁用集 + 触发 `rebuild_snapshot`；**子系统 C** 另调
  `reconcile_upstreams(...) -> ReconcileSummary` 做上游热重载）、`observe`（`CallSink`/`CallRecord`、
  `CallContentSink`/`CallContent` 与 `DiscoverySink`/`DiscoveryRecord` 契约）、`catalog`（经 `GatewaySnapshot::catalog()` 列工具）、`config`
  （装配期取上游/策略/路径；**子系统 C** 用 `Config`/`UpstreamConfig` 做 `restart_diff` 与 reconcile 基线）。
- 外部：`axum`（router/handler）、`tokio`（serve；admin handler 的 `spawn_blocking` 跑同步持久化）、
  `serde`/`serde_json`（视图序列化、JSONL 读写）、`tracing`、`subtle`（**子系统 B** admin token 的常量时间
  `ConstantTimeEq` 比较，复用下游 HTTP 的 api-key 模式）、`rust-embed`（编译期内嵌 `ui/dist/` 静态产物，`debug-embed`+`mime-guess`）。
- 前端工程在 `crates/dashboard/ui/`（Svelte 5 + Vite）：`npm run build` 重新生成 `ui/dist/`（**已入库**，故 `cargo build`
  不依赖 node；`node_modules/` gitignore），由 `assets.rs` 经 `rust-embed` 内嵌。
- discovery JSONL writer **只用 `std::thread` + `std::sync::mpsc`（有界 `sync_channel`）+ `std::fs`**（与
  `observe` 的审计 writer 同构），落盘不进 tokio 运行时。

## 被谁使用

- `mcpgw`（bin）的 `serve`：`[dashboard].enabled` 时构造 `MetricsSink`（加进**元数据 `CallSink` 切片**）与
  `CallRingSink`（加进**独立的 `CallContentSink` 切片** `content_sinks`，**不**进元数据 sinks，故内容不入
  tracing/审计/指标）、按 `trace_queries` 构造 `DiscoveryRingSink`（注入 stdio + HTTP 两个下游的 `DiscoverySink` 切片），
  并把 `[dashboard].payload_max_bytes` 一并透传给 stdio + HTTP 两个传输；面板起为**自己端口上的独立 task**
  （默认 `127.0.0.1:8971`，localhost、读端点无鉴权、admin 写经 Bearer），带优雅关停与有界 writer drain。
  **（子系统 C）** 装 `AppState` 时另注入在线改配的 6 字段——`config_path`（= `--config` 路径，缺则 config 端点 404）、
  `config_validator`（= `main.rs` 的 `validate_config_text`）、`config_write_lock`、`boot_config`、`applied_upstreams`、
  `rebuild_trigger`（`prepare_state` 返回的 trigger clone）。详见 L4 [mcpgw-main](../L4-api/mcpgw-main.md)。

## 不负责

- **进程重启 / 撤 key / 非上游热重载**：两个写子系统（B 运行时禁用、C 在线改配 + 上游热重载，均 opt-in，须配
  `admin_token_env`；C 另须 `serve --config`）覆盖临时 disable/enable 与整份 `mcpgw.toml` 编辑 + `[[upstream]]`
  增/删/改的秒级热重载，但**不**重启/管理进程、**不**撤 API-key、**不**热重载 `top_k`/`strategy`/`server`/
  `dashboard`/`audit` 等非上游字段（这些经 C 可编辑、可落盘，但回 `needs_restart` 横幅、需手动重启生效）。
  **默认未配 token 时面板纯只读**。
- **鉴权 / TLS / 反代**：开放读端点默认绑 localhost、无 auth；写子系统 B/C 的 6 个 `/api/admin/*` 经单一共享 admin
  Bearer token 鉴权（无用户系统/RBAC）。非 loopback 绑定只 `warn`，不内建 TLS（留给反代）。
- **图表库 / SSE / WebSocket**：SPA 用 **Svelte 5 + Vite** 构建、产物经 `rust-embed` 内嵌；仍每 3s 轮询
  `/api/*`、**无 SSE/WS**，也无图表库。
- **指标导出（Prometheus/OTel）**：属 `observe` 接缝的另一类 sink（M6.T2），不在本 crate。

## 向下导航

- 逐文件 API 见 L4：[dashboard](../L4-api/dashboard.md)
- 进程模型 / 算法 / 数据来源 / 隐私见 L3：[dashboard](../L3-details/dashboard.md)
- 接缝来源见：[observe L2](./observe.md) · [observe-lib L4](../L4-api/observe-lib.md) ·
  [downstream L2](./downstream.md)
- 装配入口见：[mcpgw-cli L2](./mcpgw-cli.md) · [mcpgw-main L4](../L4-api/mcpgw-main.md)
