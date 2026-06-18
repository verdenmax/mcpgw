# L4 — `crates/dashboard` API

源文件：`crates/dashboard/src/{lib,metrics,trace,history,calls,api,assets}.rs` + `ui/`（Svelte 5 + Vite
前端工程，产物内嵌）。只读可视化面板（子系统 A）：把 `gateway` 活快照、`observe` 实时观测与可选历史 JSONL
聚合成 11 个 `/api/*` JSON 端点 + 一个由 `rust-embed` 内嵌的 Svelte SPA，跑在独立 localhost 端口上。所有对外
类型经 `lib.rs` re-export。

---

## `metrics.rs`：实时指标聚合

### `struct MetricsSink`
```rust
pub struct MetricsSink { /* Mutex<MetricsState> 私有 */ }
impl observe::CallSink for MetricsSink { fn record(&self, rec: &CallRecord); }
```
内存聚合器，实现 `observe::CallSink`。`record` 把一次调用计入：

- `total += 1`；按 `rec.meta_tool.as_str()` 落入对应元工具的 `MetaAgg`（固定桶延迟直方图 + `max_ms` + 错误计数）；
- 若 `rec.upstream` 有值，落入 `per_upstream`（`OutcomeAgg { calls, errors }`）。
- **错误判定**：`is_error = !matches!(rec.outcome, CallOutcome::Ok)`——`Error` 与 **`Timeout` 都算 error**。
- **有界**：`per_meta` 键自有限元工具集；`per_upstream` 键数封顶 `const MAX_UPSTREAM_KEYS = 1024`（已存在的键
  照常累加，新键仅在 `len < MAX_UPSTREAM_KEYS` 时插入）；**总量仍全部计入** `total_calls`。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` / `Default` | `() -> Self` | 空聚合器 |
| `snapshot` | `(&self) -> MetricsSnapshot` | 拷一份当前聚合；锁中毒经 `into_inner` 容错 |

固定桶上界（ms）：`BUCKETS_MS = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 5000, u64::MAX]`（末桶无界）。
近似分位 `percentile(p)` = 累计计数首次 `>= ceil(p*calls)` 的桶上界、`min(max_ms)` 封顶（`p50 <= p95 <= max`、
绝不超实测最大；`calls == 0 → 0`）。

### `struct MetricsSnapshot` / `MetaToolMetrics` / `UpstreamMetrics`
```rust
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct MetricsSnapshot {
    pub total_calls: u64,
    pub per_meta_tool: Vec<MetaToolMetrics>,
    pub per_upstream: Vec<UpstreamMetrics>,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetaToolMetrics {
    pub meta_tool: String, pub calls: u64, pub errors: u64,
    pub p50_ms: u64, pub p95_ms: u64, pub max_ms: u64,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpstreamMetrics { pub upstream: String, pub calls: u64, pub errors: u64 }
```
`/api/metrics` 的响应体（`per_meta_tool`/`per_upstream` 按 `BTreeMap` 键有序）。

---

## `trace.rs`：发现追踪 ring + 可选 writer

### `struct TraceItem`（`Serialize`）
```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TraceItem {
    pub id: String, pub ts_unix_ms: u64, pub query: String,
    pub top_k: usize, pub results: Vec<observe::DiscoveryHit>, pub latency_ms: u64,
}
```
一条追踪经 API 暴露的形态（镜像 `calls::CallItem`）：稳定 `id`（live=十进制 ring seq；history=`"h{ts}-{n}"`）+
追踪字段。`recent`/`get`/`replay_discovery_items` 三处出口共用此类型。私有 `StoredTrace { seq, record }` 在 ring
内同时持 seq 与原始 `DiscoveryRecord`，`to_item()` 转出 `TraceItem`。

### `struct DiscoveryRingSink`
```rust
pub struct DiscoveryRingSink { /* cap, Mutex<VecDeque<StoredTrace>>, next_seq: AtomicU64, Option<SyncSender<String>>, dropped: AtomicU64 私有 */ }
impl observe::DiscoverySink for DiscoveryRingSink { fn record(&self, rec: &DiscoveryRecord); }
```
最近 discovery 追踪的有界环形缓冲（newest-first 读），可选地把每条记录 append 成一行 discovery JSONL。

| 方法 | 签名 | 说明 |
|------|------|------|
| `spawn` | `(cap: usize, path: Option<&Path>) -> io::Result<(Self, Option<DiscoveryWriter>)>` | 建 ring（容量 `cap.max(1)`）；`Some(path)` → 打开文件（create+append）+ 起命名 `discovery-writer` 的 OS 线程并返回其 `DiscoveryWriter`。文件打不开/线程起不来 → `Err` |
| `recent` | `(&self, limit: usize) -> Vec<TraceItem>` | newest-first 最多 `limit` 条（`iter().rev().take`），每条带其 live id = ring seq |
| `get` | `(&self, seq: u64) -> Option<TraceItem>` | 按 live id（十进制 seq）取单条追踪；已淘汰/从未存在 → `None`（镜像 `CallRingSink::get`） |
| `dropped_count` | `(&self) -> u64` | 因 writer channel 满被丢弃的条数（`Relaxed`，测试/诊断） |

`record`：在锁内先 `next_seq.fetch_add` 分配 seq（保证物理 ring 顺序恒等于 seq 序，避免并发 `record()` 乱序）、写
ring（满则 `pop_front`）后，若有 writer 则 `serde_json::to_string(rec)`（仍写**原始 `DiscoveryRecord`**、不含
seq/id）`try_send` 进容量 `WRITER_CHANNEL_CAP = 1024` 的有界 channel，**满则 `dropped` 计数、绝不阻塞**。

> ring 常驻内存有界：条数封顶 `cap`，每条的 `query` 已在上游 `downstream` 经 `clamp_query` 截到
> `MAX_TRACE_QUERY_CHARS = 2048` 字符，故约 `trace_buffer × 2048 字符`（不随 client 输入大小膨胀，见
> [downstream-lib](./downstream-lib.md)）。

### `struct DiscoveryWriter`
```rust
pub struct DiscoveryWriter { /* JoinHandle<()> 私有 */ }
```
| 方法 | 签名 | 说明 |
|------|------|------|
| `join` | `(self)` | 阻塞至 writer 线程 drain+flush+`sync_all`(fsync) 退出——只在**所有 `DiscoveryRingSink` clone drop**（channel 断连）后发生 |

writer loop：`recv` 一行即写 + `try_recv` 排空积压批量写 + 每批 `flush`；干净断连时**最终 flush + fsync 一次**。

---

## `history.rs`：JSONL 历史回放

### `replay_discovery_items`
```rust
pub fn replay_discovery_items(path: &Path, limit: usize) -> (Vec<TraceItem>, bool)
```
回放 discovery JSONL 为 `TraceItem`：尾扫**最后** `limit` 行（私有 `tail_lines`，内存最多 `limit` 行有界）、坏行
`from_str` 失败即跳过、末尾 `reverse()` 给 **newest-first**。每条分配稳定 id `"h{ts}-{n}"`（同 `ts` 文件序内第 n
条，与 `replay_audit_calls` 同构，故 list 分配的 id 在 detail 同窗口复现）。`DiscoveryRecord` 自身 `Deserialize`，无需
owned 镜像。`bool` = 文件可读（打不开 → `(vec![], false)`）。

### `replay_audit_metrics`
```rust
pub fn replay_audit_metrics(path: &Path, limit: usize, bucket_ms: u64) -> (Vec<MetricBucket>, bool)
```
回放审计 JSONL 进定宽时间桶（**oldest-first**）：每行只解析 `{ ts_unix_ms, outcome }`，桶起点
`ts - (ts % bucket_ms)`（`bucket_ms.max(1)`），`calls += 1`，**非 `"ok"`**（含 `"error"`/`"timeout"`）
`errors += 1`（与实时 `MetricsSink` 口径一致）。坏行跳过。`bool` = 文件可读。

### `struct MetricBucket`
```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MetricBucket { pub bucket_start_ms: u64, pub calls: u64, pub errors: u64 }
```
一个定宽时间桶的审计指标，`/api/metrics/history` 的元素。

### `replay_audit_calls`
```rust
pub fn replay_audit_calls(path: &Path, scan_limit: usize, filter: &CallFilter) -> (Vec<CallItem>, bool)
```
把 audit JSONL 反序列化为 `CallItem`（owned 镜像 `AuditCallLine`，因 `CallRecord` 不可反序列化），newest-first，扫描至多末尾 `scan_limit` 行；坏行跳过；id 为 `"h{ts}-{n}"`（同 `ts` 文件序内第 n 条，稳定）；`filter` 在 id 分配后应用。`bool` = 文件可读。

---

## `calls.rs`：逐条调用环 + 统一项类型

### `struct CallItem`（`Serialize`）
live 环与 history 回放共用的 owned 项：`id`（live=十进制 seq；history=`"h{ts}-{n}"`）、`ts_unix_ms`、`meta_tool`、`target_tool?`、`upstream?`、`latency_ms`、`outcome`、`error_kind?`、`arg_bytes`、`result_bytes`，外加可选**内容**字段 `args?`/`args_truncated`/`result?`/`result_truncated`（M1 调用内容捕获）。内容字段**仅详情**填充：`get(seq)`（→`/api/calls/{id}`）以 `with_content=true` 带出 args/result；列表 `query`（→`/api/calls`）以 `with_content=false` **省略**它们（`args`/`result` 为 `None` 经 `skip_serializing_if` 不出现，`*_truncated=false` 同样不序列化）。history 回放项内容恒 `None`（审计仅元数据）。

### `struct CallFilter`
`meta_tool`/`upstream`/`target_tool`/`outcome`/`since_ms`/`until_ms`，均 `Option`（`None`=全匹配；`since_ms`/`until_ms` 为闭区间，含端点）；`matches(&CallItem)` 对 live 与 history 两数据源统一过滤。

### `struct CallRingSink`（实现 `observe::CallContentSink`）
有界内存环（满淘汰最旧，镜像 `DiscoveryRingSink`），每条插入在锁内分配单调 `seq` 作 live id。环内每条是 `StoredCall { seq, record: CallRecord, content: CallContent }`——**同时**存元数据与内容（一条记录富含两者，不重复元数据字段）；私有 `to_item(with_content: bool)` 转出 `CallItem`，`with_content=false` 时内容四字段留空（列表用），`true` 时带出 args/result（详情用）。`record(&self, meta: &CallRecord, content: &CallContent)` 实现 `CallContentSink`：克隆 `meta`+`content` 入环。`query(&CallFilter, limit, offset) -> (Vec<CallItem>, total)` newest-first（`to_item(false)`、`total` 计全部命中）；`get(seq) -> Option<CallItem>`（`to_item(true)`）。容量 = `[dashboard].call_buffer`；单条 args/result 的字节上界由下游按 `[dashboard].payload_max_bytes`（默认 16384）截断后才入环，故常驻内存按 `call_buffer × payload_max_bytes` 有界（重启即丢）。

---

## `api.rs`：状态、视图类型与纯函数

### `struct AppState`
```rust
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<gateway::GatewayState>,
    pub metrics: Arc<MetricsSink>,
    pub discovery: Option<Arc<DiscoveryRingSink>>,
    pub calls: Option<Arc<CallRingSink>>,
    pub upstreams: Vec<UpstreamInfo>,
    pub strategy: String,
    pub audit_path: Option<PathBuf>,
    pub discovery_path: Option<PathBuf>,
    pub started_at: Instant,
}
```
handler 的**只读共享态**（装配期一次性注入）：活快照句柄 + 实时指标 sink + 可选 discovery ring +
可选 per-call ring（`calls`，仅 dashboard 启用时 `Some`）+ 配置上游列表/策略名 + 历史 JSONL 路径 + 启动时刻。

### `struct UpstreamInfo`
```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpstreamInfo { pub name: String, pub transport: String }
```
一个配置上游的静态身份（`transport` 由 `mcpgw` 的 `transport_str` 给出 `"stdio"`/`"http"`）。

### 视图类型（均 `Serialize`）
```rust
pub struct Overview { uptime_secs, strategy, upstreams_total, upstreams_connected, tools_total, total_calls, last_rebuild_skipped }
pub struct UpstreamView { name, transport, status: &'static str /* "connected"|"skipped"|"unknown" */, reason: Option<String>, tools, calls, errors }
pub struct UpstreamDetail { name, transport, status: &'static str, reason: Option<String>, tools_count, calls, errors, tools: Vec<ToolView> }
pub struct ToolView { name, description }
pub struct ToolDetail { name, server, description, input_schema: serde_json::Value }
pub struct TracesResponse { source, history_unavailable, traces: Vec<TraceItem> }
pub struct HistoryResponse { history_unavailable, buckets: Vec<MetricBucket> }
pub struct CallsResponse { source, history_unavailable, total, items: Vec<CallItem> }
```
（`TraceItem` 定义见 [`trace.rs`](#tracers发现追踪-ring--可选-writer)；`UpstreamDetail` 是 `UpstreamView` 各字段 +
其当前工具列表 `Vec<ToolView>`；`ToolDetail` 加 `server`（所属上游）+ `input_schema`。）

### 纯函数（handler 调用）

| 函数 | 签名 | 说明 |
|------|------|------|
| `overview` | `(&AppState) -> Overview` | `catalog().len()` 工具数、`MetricsSnapshot.total_calls`、`last_summary().skipped.len()`、上游总数/连接数、`started_at.elapsed()` |
| `upstreams` | `(&AppState) -> Vec<UpstreamView>` | 每配置上游：状态由 `last_summary` 定（`ingested`→`connected`、`skipped`→`skipped`+原因、无 summary→`unknown`），工具数按 `catalog()` 的 `server` 过滤，调用/错误取 `per_upstream` |
| `upstream_detail` | `(&AppState, name) -> Option<UpstreamDetail>` | `name` 非配置上游→`None`；否则 `UpstreamView` 各字段 + 按 `catalog()` 的 `server` 过滤出的工具列表（`ToolView` 的 `qualified_name`/`description`），`tools_count` = 列表长度 |
| `tools` | `(&AppState, q: Option<&str>) -> Vec<ToolView>` | 列 `catalog()`；非空 `q` 时对 `qualified_name`/`description` 小写 `contains` 过滤 |
| `tool_detail` | `(&AppState, name) -> Option<ToolDetail>` | 按 qualified `{server}__{tool}` 在 `catalog().get(name)` 取；缺→`None`；带 `server` 与 `input_schema` |
| `metrics` | `(&AppState) -> MetricsSnapshot` | `metrics.snapshot()` |
| `traces` | `(&AppState, limit, source) -> TracesResponse` | `source=="history"` → `replay_discovery_items(discovery_path, TRACE_HISTORY_SCAN)` 后 `truncate(limit)`（无 path→`history_unavailable`）；否则 `discovery.recent(limit)`（未启用→空、`history_unavailable=false`）。元素为 `TraceItem` |
| `trace_detail` | `(&AppState, id) -> Option<TraceItem>` | `is_history_id(id)` → 重扫 `TRACE_HISTORY_SCAN` 行回放后按 id 定位；否则按十进制 live seq 取环 `get(seq)`；找不到/源不可用→`None` |
| `metrics_history` | `(&AppState, limit, bucket_ms) -> HistoryResponse` | `replay_audit_metrics(audit_path)`（无 path→`history_unavailable`） |
| `call_filter_from_query` | `(&HashMap<String,String>) -> CallFilter` | 从查询参数 `meta`/`upstream`/`tool`（→`target_tool`）/`outcome`/`since`/`until` 构造过滤器（`since`/`until` 解析为 `u64` ms） |
| `calls` | `(&AppState, &CallFilter, source, scan_limit, limit, offset) -> CallsResponse` | `source=="history"` → `replay_audit_calls(audit_path, scan_limit)`（无 path→`history_unavailable`、`total`=全部命中、`skip(offset).take(limit)`）；否则 `calls.query(filter, limit, offset)`（未启用→空、`history_unavailable=false`） |
| `call_detail` | `(&AppState, id) -> Option<CallItem>` | `is_history_id(id)` → 重扫 `CALL_HISTORY_SCAN` 行回放后按 id 定位；否则按十进制 live seq 取环 `get(seq)`；找不到/源不可用→`None` |
| `is_history_id` | `(id: &str) -> bool` | id 以 `h` 开头即历史（`"h{ts}-{n}"`）；否则按 live ring 十进制 seq。格式判定集中于此，使 handler 的 blocking-pool 决策与 `call_detail`/`trace_detail` 源路由不漂移 |

> `const CALL_HISTORY_SCAN = 50_000`：list-history（`calls` 的 `source=history`）与单条 `call_detail` **共用同一扫描窗口**，故 list 分配的 `"h{ts}-{n}"` id 在 detail 里用同一窗口稳定复现、不漂移。`const TRACE_HISTORY_SCAN = 50_000` 对 `traces`/`trace_detail` 同理（镜像 `CALL_HISTORY_SCAN`）。

---

## `assets.rs`：内嵌 UI 资源（rust-embed）

### `struct Assets`
```rust
#[derive(RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;
```
把构建好的 Svelte 前端（`ui/dist/`，见下「前端 SPA」）在**编译期**整目录内嵌进二进制。`rust-embed` 开
`debug-embed`（debug 构建也内嵌，不靠运行时读盘）+ `mime-guess`（按扩展名推 MIME）两个 feature。`ui/dist/`
**已入库**，故 `cargo build` 无需 node 工具链——前端产物已是仓库内的静态文件。

### `static_handler`
```rust
pub async fn static_handler(uri: Uri) -> Response
```
按请求路径回内嵌资源，作为 router 的 `.fallback` 挂在所有 `/api/*` 路由之后（凡未命中 `/api/*` 的请求都落到它）：

- `/`（空路径）→ 内嵌 `index.html`（`Content-Type: text/html`）；
- `/assets/*` → 该内嵌资源，`Content-Type` 取 `content.metadata.mimetype()`（hash 命名的 JS/CSS）；
- 未知路径 → **回退** `index.html`（无害：SPA 用 hash 路由，真实请求只有 `/` 与 `/assets/*`，此回退只让误打的深链刷新仍加载 app）；
- 连 `index.html` 都取不到 → `404`。

---

## `lib.rs`：路由装配

### `build_dashboard_router`
```rust
pub fn build_dashboard_router(state: Arc<AppState>, enforce_loopback_host: bool) -> axum::Router
```
装配面板 router（`with_state(state)`）。当 `enforce_loopback_host` 为 `true`（面板绑 loopback）时，额外
`layer` 一层 `require_local_host` 中间件以关闭 DNS 重绑定向量；为 `false`（绑非 loopback 的显式外网暴露）时
不挂该层：

| 方法 | 路由 | handler → 响应 |
|------|------|----------------|
| GET | `/api/overview` | `Json<Overview>` |
| GET | `/api/upstreams` | `Json<Vec<UpstreamView>>` |
| GET | `/api/upstreams/{name}` | `Json<UpstreamDetail>` 或 404（其工具列表 + 计数/状态） |
| GET | `/api/tools?q=` | `Json<Vec<ToolView>>` |
| GET | `/api/tools/{name}` | `Json<ToolDetail>` 或 404（schema + 所属上游；`name`=qualified `{server}__{tool}`） |
| GET | `/api/metrics` | `Json<MetricsSnapshot>` |
| GET | `/api/traces?source=live\|history&limit=` | `Json<TracesResponse>`（`limit` 缺省 100、`min(MAX_HISTORY_LIMIT=50_000)`；`source` 缺省 `"live"`） |
| GET | `/api/traces/{id}` | `Json<TraceItem>` 或 404（`h…`→历史回放定位；否则 live seq 取环） |
| GET | `/api/metrics/history?limit=&bucket_ms=` | `Json<HistoryResponse>`（`limit` 缺省 5000、封顶 50_000；`bucket_ms` 缺省 60_000） |
| GET | `/api/calls?source=live\|history&meta=&upstream=&tool=&outcome=&since=&until=&limit=&offset=` | `Json<CallsResponse>`（`limit` 缺省 100、`min(MAX_HISTORY_LIMIT=50_000)`；`offset` 缺省 0；`source` 缺省 `"live"`；history 路径走 `spawn_blocking`） |
| GET | `/api/calls/{id}` | `Json<CallItem>` 或 404（`h…`→历史回放定位；否则按 live seq 取环） |
| —（fallback） | 任意非 `/api/*` 路径 | `assets::static_handler`：`/`→内嵌 `index.html`、`/assets/*`→内嵌资源（带 hash），其余回退 index |

私有 `qparam_usize(q, key, default)` 解析查询参数；`const MAX_HISTORY_LIMIT = 50_000` 封顶历史 `limit`。
静态资源经 `assets::static_handler`（router `.fallback`）从 `rust-embed` 内嵌的 `ui/dist/` 交付（见
[`assets.rs`](#assetsrs内嵌-ui-资源rust-embed)）。

### Host 头校验：`host_is_local` / `require_local_host`（私有）
```rust
fn host_is_local(host: Option<&str>) -> bool
async fn require_local_host(req: Request, next: Next) -> axum::response::Response
```
抗 DNS 重绑定的防线（**非鉴权**）：面板无鉴权、靠绑 loopback 控制访问，但远端页面可把自家域名重绑到
`127.0.0.1` 后同源 `fetch /api/*`——它仍发自家域名的 `Host`，故可据 `Host` 拦下。`require_local_host` 把
`Host` 非本地的请求一律 `403`，**仅当** `build_dashboard_router` 的 `enforce_loopback_host == true`（绑 loopback）
时挂载；绑非 loopback 则跳过。`host_is_local`：剥端口、处理 IPv6 `[::1]`，`localhost`（忽略大小写）/回环 IP
判为本地；含 `@`（userinfo）的 Host 防御性直接拒（合法 `Host` 永不含 `@`），缺/不可解析 Host → 非本地。

### 前端 SPA（`ui/`：Svelte 5 + Vite，产物内嵌）
源在 `crates/dashboard/ui/src/`，`npm run build` 经 Vite 产出 `ui/dist/`（hash 命名的多文件，**已入库**），再由
[`assets.rs`](#assetsrs内嵌-ui-资源rust-embed) 经 `rust-embed` 编译期内嵌，故 `cargo build` 不依赖 node。

- **左侧导航**（`Nav.svelte`）：Overview / Upstreams / Tools / Calls / Traces。
- **hash 路由**（`router.svelte.js`）：`#/<view>/<...params>`（如 `#/calls`、`#/calls/{id}`、`#/upstreams/{name}`、
  `#/tools/{name}`、`#/traces/{id}`）；params 经 `decodeURIComponent` 解码（含 `__`/编码字符的 qualified 工具名安全），
  fragment 不发往服务端，故深链刷新只请求 `/`（不需要 history 回退改写）。
- 各视图每 3s 轮询既有 `/api/*` 端点（`api.js` 的 `getJSON`）。**Calls 页**用 `/api/metrics` 的可点击指标卡过滤
  `/api/calls` 逐条列表，行点击进 `/api/calls/{id}` 详情下钻。
- **三个详情视图**（M3）：`UpstreamDetail`（`/api/upstreams/{name}`：状态/计数 + 该上游工具表）、`ToolDetail`
  （`/api/tools/{name}`：schema + 所属上游）、`TraceDetail`（`/api/traces/{id}`：query + 命中列表）。`Overview` 卡片
  可点直达对应列表（upstreams/tools/calls）。
- **交叉链接**形成 列表→详情→跳转 闭环：上游↔工具（`UpstreamDetail`↔`ToolDetail`）、工具/上游↔调用
  （`*Detail`→`/api/calls?tool=/upstream=` 近期调用、`CallDetail`→工具/上游）、追踪→命中工具（`TraceDetail`→
  `ToolDetail`）；href 一律 `encodeURIComponent` 编码名/ id。
- **XSS 防线**：Svelte 的 `{expr}` 插值**自动转义**，全前端**不用** `{@html}`（`assets.rs` 的
  `no_svelte_component_uses_raw_html` 单测扫描 `ui/src` 锁死这一点），故 query 文本/工具名/上游名/错误原因等不可信
  字段绝不以原始 HTML 注入。

## 依赖与扩展点

- `MetricsSink` / `DiscoveryRingSink` 是 `observe::CallSink` / `observe::DiscoverySink` 接缝的**两个独立
  实现**：前者读仅元数据 `CallRecord`、后者读 opt-in `DiscoveryRecord`，二者物理隔离（隐私边界见 L3）。
- discovery JSONL writer 只用 `std::thread` + `std::sync::mpsc`（有界 `sync_channel`）+ `std::fs`，**不进
  tokio 运行时**（与 `observe` 审计 writer 同构）。
- 装配/关停顺序见 [mcpgw-main](./mcpgw-main.md)；配置见 [config-lib](./config-lib.md) 的 `DashboardConfig`。

> 进程模型/算法/数据来源/隐私见 L3：[dashboard](../L3-details/dashboard.md)；组件视角见 L2：
> [dashboard](../L2-components/dashboard.md)。
