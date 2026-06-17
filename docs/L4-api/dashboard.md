# L4 — `crates/dashboard` API

源文件：`crates/dashboard/src/{lib,metrics,trace,history,calls,api,assets}.rs` + `ui/`（Svelte 5 + Vite
前端工程，产物内嵌）。只读可视化面板（子系统 A）：把 `gateway` 活快照、`observe` 实时观测与可选历史 JSONL
聚合成 8 个 `/api/*` JSON 端点 + 一个由 `rust-embed` 内嵌的 Svelte SPA，跑在独立 localhost 端口上。所有对外
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

### `struct DiscoveryRingSink`
```rust
pub struct DiscoveryRingSink { /* cap, Mutex<VecDeque<DiscoveryRecord>>, Option<SyncSender<String>>, AtomicU64 私有 */ }
impl observe::DiscoverySink for DiscoveryRingSink { fn record(&self, rec: &DiscoveryRecord); }
```
最近 discovery 追踪的有界环形缓冲（newest-first 读），可选地把每条记录 append 成一行 discovery JSONL。

| 方法 | 签名 | 说明 |
|------|------|------|
| `spawn` | `(cap: usize, path: Option<&Path>) -> io::Result<(Self, Option<DiscoveryWriter>)>` | 建 ring（容量 `cap.max(1)`）；`Some(path)` → 打开文件（create+append）+ 起命名 `discovery-writer` 的 OS 线程并返回其 `DiscoveryWriter`。文件打不开/线程起不来 → `Err` |
| `recent` | `(&self, limit: usize) -> Vec<DiscoveryRecord>` | newest-first 最多 `limit` 条（`iter().rev().take`） |
| `dropped_count` | `(&self) -> u64` | 因 writer channel 满被丢弃的条数（`Relaxed`，测试/诊断） |

`record`：写 ring（满则 `pop_front`）后，若有 writer 则 `serde_json::to_string(rec)` `try_send` 进容量
`WRITER_CHANNEL_CAP = 1024` 的有界 channel，**满则 `dropped` 计数、绝不阻塞**。

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

### `replay_discovery`
```rust
pub fn replay_discovery(path: &Path, limit: usize) -> (Vec<DiscoveryRecord>, bool)
```
回放 discovery JSONL：尾扫**最后** `limit` 行（私有 `tail_lines`，内存最多 `limit` 行有界）、坏行
`from_str` 失败即跳过、末尾 `reverse()` 给 **newest-first**。`bool` = 文件可读（打不开 → `(vec![], false)`）。

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
live 环与 history 回放共用的 owned 项：`id`（live=十进制 seq；history=`"h{ts}-{n}"`）、`ts_unix_ms`、`meta_tool`、`target_tool?`、`upstream?`、`latency_ms`、`outcome`、`error_kind?`、`arg_bytes`、`result_bytes`。仅元数据。

### `struct CallFilter`
`meta_tool`/`upstream`/`target_tool`/`outcome`/`since_ms`/`until_ms`，均 `Option`（`None`=全匹配；`since_ms`/`until_ms` 为闭区间，含端点）；`matches(&CallItem)` 对 live 与 history 两数据源统一过滤。

### `struct CallRingSink`（实现 `observe::CallSink`）
有界内存环（满淘汰最旧，镜像 `DiscoveryRingSink`），每条插入在锁内分配单调 `seq` 作 live id。`query(&CallFilter, limit, offset) -> (Vec<CallItem>, total)` newest-first、`total` 计全部命中；`get(seq) -> Option<CallItem>`。容量 = `[dashboard].call_buffer`。

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
pub struct ToolView { name, description }
pub struct TracesResponse { source, history_unavailable, traces: Vec<observe::DiscoveryRecord> }
pub struct HistoryResponse { history_unavailable, buckets: Vec<MetricBucket> }
pub struct CallsResponse { source, history_unavailable, total, items: Vec<CallItem> }
```

### 纯函数（handler 调用）

| 函数 | 签名 | 说明 |
|------|------|------|
| `overview` | `(&AppState) -> Overview` | `catalog().len()` 工具数、`MetricsSnapshot.total_calls`、`last_summary().skipped.len()`、上游总数/连接数、`started_at.elapsed()` |
| `upstreams` | `(&AppState) -> Vec<UpstreamView>` | 每配置上游：状态由 `last_summary` 定（`ingested`→`connected`、`skipped`→`skipped`+原因、无 summary→`unknown`），工具数按 `catalog()` 的 `server` 过滤，调用/错误取 `per_upstream` |
| `tools` | `(&AppState, q: Option<&str>) -> Vec<ToolView>` | 列 `catalog()`；非空 `q` 时对 `qualified_name`/`description` 小写 `contains` 过滤 |
| `metrics` | `(&AppState) -> MetricsSnapshot` | `metrics.snapshot()` |
| `traces` | `(&AppState, limit, source) -> TracesResponse` | `source=="history"` → `replay_discovery(discovery_path)`（无 path→`history_unavailable`）；否则 `discovery.recent(limit)`（未启用→空、`history_unavailable=false`） |
| `metrics_history` | `(&AppState, limit, bucket_ms) -> HistoryResponse` | `replay_audit_metrics(audit_path)`（无 path→`history_unavailable`） |
| `call_filter_from_query` | `(&HashMap<String,String>) -> CallFilter` | 从查询参数 `meta`/`upstream`/`tool`（→`target_tool`）/`outcome`/`since`/`until` 构造过滤器（`since`/`until` 解析为 `u64` ms） |
| `calls` | `(&AppState, &CallFilter, source, scan_limit, limit, offset) -> CallsResponse` | `source=="history"` → `replay_audit_calls(audit_path, scan_limit)`（无 path→`history_unavailable`、`total`=全部命中、`skip(offset).take(limit)`）；否则 `calls.query(filter, limit, offset)`（未启用→空、`history_unavailable=false`） |
| `call_detail` | `(&AppState, id) -> Option<CallItem>` | `is_history_id(id)` → 重扫 `CALL_HISTORY_SCAN` 行回放后按 id 定位；否则按十进制 live seq 取环 `get(seq)`；找不到/源不可用→`None` |
| `is_history_id` | `(id: &str) -> bool` | id 以 `h` 开头即历史（`"h{ts}-{n}"`）；否则按 live ring 十进制 seq。格式判定集中于此，使 handler 的 blocking-pool 决策与 `call_detail` 源路由不漂移 |

> `const CALL_HISTORY_SCAN = 50_000`：list-history（`calls` 的 `source=history`）与单条 `call_detail` **共用同一扫描窗口**，故 list 分配的 `"h{ts}-{n}"` id 在 detail 里用同一窗口稳定复现、不漂移。

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
| GET | `/api/tools?q=` | `Json<Vec<ToolView>>` |
| GET | `/api/metrics` | `Json<MetricsSnapshot>` |
| GET | `/api/traces?source=live\|history&limit=` | `Json<TracesResponse>`（`limit` 缺省 100、`min(MAX_HISTORY_LIMIT=50_000)`；`source` 缺省 `"live"`） |
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
- **hash 路由**（`router.svelte.js`）：`#/<view>/<...params>`（如 `#/calls`、`#/calls/{id}`）；fragment 不发往
  服务端，故深链刷新只请求 `/`（不需要 history 回退改写）。
- 各视图每 3s 轮询既有 `/api/*` 端点（`api.js` 的 `getJSON`）。**Calls 页**用 `/api/metrics` 的可点击指标卡过滤
  `/api/calls` 逐条列表，行点击进 `/api/calls/{id}` 详情下钻。
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
