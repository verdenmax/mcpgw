# L4 — `crates/dashboard` API

源文件：`crates/dashboard/src/{lib,metrics,trace,history,api}.rs` + `assets/`。只读可视化面板（子系统 A）：
把 `gateway` 活快照、`observe` 实时观测与可选历史 JSONL 聚合成 6 个 `/api/*` JSON 端点 + 一个零构建原生 JS
SPA，跑在独立 localhost 端口上。所有对外类型经 `lib.rs` re-export。

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

---

## `api.rs`：状态、视图类型与纯函数

### `struct AppState`
```rust
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<gateway::GatewayState>,
    pub metrics: Arc<MetricsSink>,
    pub discovery: Option<Arc<DiscoveryRingSink>>,
    pub upstreams: Vec<UpstreamInfo>,
    pub strategy: String,
    pub audit_path: Option<PathBuf>,
    pub discovery_path: Option<PathBuf>,
    pub started_at: Instant,
}
```
handler 的**只读共享态**（装配期一次性注入）：活快照句柄 + 实时指标 sink + 可选 discovery ring +
配置上游列表/策略名 + 历史 JSONL 路径 + 启动时刻。

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

---

## `lib.rs`：路由装配

### `build_dashboard_router`
```rust
pub fn build_dashboard_router(state: Arc<AppState>) -> axum::Router
```
装配面板 router（`with_state(state)`）：

| 方法 | 路由 | handler → 响应 |
|------|------|----------------|
| GET | `/api/overview` | `Json<Overview>` |
| GET | `/api/upstreams` | `Json<Vec<UpstreamView>>` |
| GET | `/api/tools?q=` | `Json<Vec<ToolView>>` |
| GET | `/api/metrics` | `Json<MetricsSnapshot>` |
| GET | `/api/traces?source=live\|history&limit=` | `Json<TracesResponse>`（`limit` 缺省 100、`min(MAX_HISTORY_LIMIT=50_000)`；`source` 缺省 `"live"`） |
| GET | `/api/metrics/history?limit=&bucket_ms=` | `Json<HistoryResponse>`（`limit` 缺省 5000、封顶 50_000；`bucket_ms` 缺省 60_000） |
| GET | `/` | `Html(INDEX_HTML)`（内嵌 `assets/index.html`） |
| GET | `/app.js` | `application/javascript`（内嵌 `assets/app.js`） |
| GET | `/style.css` | `text/css`（内嵌 `assets/style.css`） |

私有 `qparam_usize(q, key, default)` 解析查询参数；`const MAX_HISTORY_LIMIT = 50_000` 封顶历史 `limit`。
静态资源经 `include_str!` 内嵌进二进制（零外部文件依赖）。

### SPA（`assets/app.js` + `index.html` + `style.css`）
零依赖原生 JS，每 `REFRESH_MS = 3000` 轮询 `/api/overview`、`/api/upstreams`、`/api/metrics`、
`/api/traces`。**所有不可信字段**（`r.query`、`h.name`、`u.reason`、`u.name`、`u.transport`、`x.meta_tool`）
经 `escapeHtml` 后才写入 `innerHTML`，防 stored XSS（crate 单测锁死这六处转义）。

## 依赖与扩展点

- `MetricsSink` / `DiscoveryRingSink` 是 `observe::CallSink` / `observe::DiscoverySink` 接缝的**两个独立
  实现**：前者读仅元数据 `CallRecord`、后者读 opt-in `DiscoveryRecord`，二者物理隔离（隐私边界见 L3）。
- discovery JSONL writer 只用 `std::thread` + `std::sync::mpsc`（有界 `sync_channel`）+ `std::fs`，**不进
  tokio 运行时**（与 `observe` 审计 writer 同构）。
- 装配/关停顺序见 [mcpgw-main](./mcpgw-main.md)；配置见 [config-lib](./config-lib.md) 的 `DashboardConfig`。

> 进程模型/算法/数据来源/隐私见 L3：[dashboard](../L3-details/dashboard.md)；组件视角见 L2：
> [dashboard](../L2-components/dashboard.md)。
