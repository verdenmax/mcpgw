# L4 — `crates/dashboard` API

源文件：`crates/dashboard/src/{lib,metrics,trace,history,calls,api,assets}.rs` + `ui/`（Svelte 5 + Vite
前端工程，产物内嵌）。只读可视化面板（子系统 A）：把 `gateway` 活快照、`observe` 实时观测与可选历史 JSONL
聚合成 13 个 `/api/*` JSON 端点 + 一个由 `rust-embed` 内嵌的 Svelte SPA，跑在独立 localhost 端口上。所有对外
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

**内容过滤参数**（M2，**仅 `source=live`**）：
- `q`（自由文本）：对 args + result 的（截断后）JSON 文本做**大小写不敏感子串**匹配（`content_contains`）。因扫的是原始 JSON 文本，子串可命中 JSON **键名**与标点/语法，而不限于值。
- `arg_key` + `arg_val`（结构化）：解析 args JSON，**递归**查找任一键 `== arg_key`（精确、大小写敏感）且其**字符串化**值 `contains` `arg_val`（大小写不敏感）的项（`args_key_value_matches`）。容器值（对象/数组）会被字符串化，故值匹配可命中序列化后的容器文本。两者**必须成对**给出；只给其一（仅 key 或仅 val）被静默忽略（no-op）。截断/非法 args JSON → 不命中（best-effort）。
- 内容过滤**仅作用于带内容的项**：`matches` 把内容检查门控在 `if let Some(args)` 之内，故 history 回放项（`args == None`）与列表轻量项永不被内容过滤排除——`source=history` 时内容过滤被静默忽略。

### `struct CallRingSink`（实现 `observe::CallContentSink`）
有界内存环（满淘汰最旧，镜像 `DiscoveryRingSink`），每条插入在锁内分配单调 `seq` 作 live id。环内每条是 `StoredCall { seq, record: CallRecord, content: CallContent }`——**同时**存元数据与内容（一条记录富含两者，不重复元数据字段）；私有 `to_item(with_content: bool)` 转出 `CallItem`，`with_content=false` 时内容四字段留空（列表用），`true` 时带出 args/result（详情用）。`record(&self, meta: &CallRecord, content: &CallContent)` 实现 `CallContentSink`：克隆 `meta`+`content` 入环。`query(&CallFilter, limit, offset) -> (Vec<CallItem>, total)` newest-first（`total` 计全部命中）；`get(seq) -> Option<CallItem>`（`to_item(true)`）。容量 = `[dashboard].call_buffer`；单条 args/result 的字节上界由下游按 `[dashboard].payload_max_bytes`（默认 16384）截断后才入环，故常驻内存按 `call_buffer × 2 × payload_max_bytes` 有界（args 与 result 各自封顶 `payload_max_bytes`，重启即丢）。

**`query` 内容过滤性能（metadata-first）**：仅当存在内容过滤（`q` 或成对的 `arg_key`+`arg_val`）时 `want_content` 为真。每条先建轻量项 `to_item(false)` 跑一遍 `matches`（此时只校验元数据，内容检查因缺 `Some(args)` 被跳过）；无内容过滤时直接返回轻量项（与 M1 同成本）；有内容过滤时**仅对通过元数据谓词的幸存者**再建带内容项 `to_item(true)` 复跑 `matches`（此时才施加内容过滤）。随后做分页，并在返回前**剥离**页内内容——列表响应**永不含内容**（仅单条详情 `/api/calls/{id}` 带 args/result）。

**`activity(&self, window_ms) -> activity::ActivityResponse`**（M3 活动聚合）：在锁内把环内每条 `StoredCall` 的**元数据**（`seq`→`id`、`ts_unix_ms`、`meta_tool`、`target_tool?`、`latency_ms`、`outcome`、`error_kind?`）投影为 `activity::AggInput`（**绝不读 `content`**），释放锁后交给纯函数 `activity::aggregate(&inputs, window_ms, now)`（`now = CallRecord::now_unix_ms()`）。投影是 owned 拷贝，使聚合与环内部解耦、`aggregate` 可独立单测；窗内/窗外统一由 `aggregate` 按窗过滤。

---

## `activity.rs`：活动聚合（只读、仅元数据）

把 live 调用环窗内记录聚合为 dashboard 的趋势 sparkline + error_kind 分布 + 最慢/最忙 Top-N。**纯函数**不依赖环内部结构，便于单测；**隐私**：`ActivityResponse` 类型不含任何 `args`/`result` 字段（单测 `response_has_no_payload_content_fields` / `activity_aggregates_live_ring_window` 断言序列化无 `"args"`/`"result"`）。

- `const BUCKETS: usize = 24`（sparkline 固定桶数，柱数恒定渲染稳定）；`const TOP_N: usize = 5`（排行榜/分布 Top-N）。

### `struct AggInput`
从一条 ring 记录投影出的 owned 聚合输入：`id`、`ts_unix_ms`、`meta_tool`、`target_tool: Option<String>`、`latency_ms`、`outcome`（`"ok"|"error"|"timeout"`）、`error_kind: Option<String>`。**仅元数据**，无内容字段。

### 响应类型（均 `Serialize`；仅 `ActivityResponse` 经 `lib.rs` `pub use` 暴露到 crate 根）
```rust
pub struct ActivityResponse { window_ms, bucket_ms, buckets: Vec<ActivityBucket>, total, errors, by_error_kind: Vec<KindCount>, slowest: Vec<SlowCall>, busiest_tools: Vec<ToolCount> }
pub struct ActivityBucket { t, total, errors }            // 桶起点 ts + 桶内总数/错误数
pub struct KindCount { kind, count }                      // error_kind 分布项
pub struct SlowCall { id, label, meta_tool, latency_ms, outcome }  // label = target_tool 否则回退 meta_tool
pub struct ToolCount { name, count }                      // busiest_tools 项（name = target_tool）
```

### `aggregate(inputs: &[AggInput], window_ms: u64, now: u64) -> ActivityResponse`
- **固定 24 桶**：`bucket_ms = (window_ms / BUCKETS).max(1)`、`span = bucket_ms * 24`、`start = now.saturating_sub(span)`；桶 `i` 起点 `t = start + i*bucket_ms`，`now` 落**末桶**。
- **窗外不计**：`ts_unix_ms < start` 跳过；桶索引 `((ts - start) / bucket_ms).min(BUCKETS-1)`（先在 u64 内 clamp 再转 usize，防远未来 ts 越界）。
- `errors`：`outcome != "ok"`（`error`/`timeout` 均计错；同时累加到该桶 `errors`）。
- `by_error_kind`：仅 `error_kind` 为 `Some` 的计数；按 count 降序、并列 kind 名升序。
- `busiest_tools`：**仅 `target_tool`** 计数（`search_tools` 无 target 不计）；count 降序、并列名升序，取前 `TOP_N`。
- `slowest`：按 `latency_ms` 降序、并列按 `ts` 降序（更晚在前），取前 `TOP_N`；`label` 取 `target_tool`，无则回退 `meta_tool`。
- 空输入 → 24 个全 0 桶、`total=0`。

---

## `about.rs`：About/Settings 只读视图（启动时组装、仅非敏感）

启动时由 `AboutInfo::from_config(&config::Config, VersionInfo)` 从生效配置 + 版本组装一次、存进 `AppState.about`，`/api/about` 直接序列化（运行期不变、零计算）。**隐私边界**：`AboutInfo` 及其嵌套类型**字段集里根本不含**任何密钥/token/env 名/env 值/上游认证引用——`http_auth` **仅 bool**（有无 `api_key`），绝不带键名/env。单测 `http_auth_true_and_no_secrets_leak` 断言序列化 JSON 不含 `SECRET_KEY`/`REMOTE_TOKEN`/`bearer_env`/`api_key` 等。

### 类型（均 `Serialize, Clone`；仅 `AboutInfo`/`VersionInfo` 经 `lib.rs` `pub use` 暴露到 crate 根）
```rust
pub struct AboutInfo { version: VersionInfo, retrieval: RetrievalInfo, dashboard: DashboardInfo, audit: AuditInfo, server: ServerInfo, upstreams: Vec<UpstreamConfigInfo> }
pub struct VersionInfo { version: String, git_sha: String, build_time: String }   // build_time = epoch 秒字符串
pub struct RetrievalInfo { strategy: String, top_k: usize }
pub struct DashboardInfo { call_buffer: usize, payload_max_bytes: usize, trace_queries: bool, trace_buffer: usize, trace_path: Option<String> }
pub struct AuditInfo { enabled: bool, path: Option<String> }                       // path 仅 enabled 时 Some
pub struct ServerInfo { stdio: bool, http_enabled: bool, http_bind: Option<String>, http_path: Option<String>, http_auth: bool }
pub struct UpstreamConfigInfo { name: String, transport: String, call_timeout_ms: u64 }
```
- `version` 由 `main.rs` 注入（`CARGO_PKG_VERSION` + `build.rs` 写的 `MCPGW_GIT_SHA`/`MCPGW_BUILD_TIME`，见 [`mcpgw-main.md`](mcpgw-main.md)）。
- `server`：`http_enabled`/`http_bind`/`http_path` 取自 `cfg.server.http`（仅当存在且 `enabled`，否则 `false`/`None`）；`http_auth = !api_keys.is_empty()`（**仅** bool）。
- `upstreams`：每配置上游映射 `name` + `transport`（`transport_label`）+ `call_timeout_ms`，**不含** url/bearer_env 等。

### `AboutInfo::from_config(cfg: &config::Config, version: VersionInfo) -> AboutInfo`
纯函数：仅拷贝非敏感字段。`audit.path` 仅 `cfg.audit.enabled` 时 `Some`；`server.http_*` 见上。

### `transport_label(&config::UpstreamTransport) -> &'static str`（私有）
`Stdio{..}` → `"stdio"`、`Http{..}` → `"http"`。**自包含**（不复用 `mcpgw` 的 `transport_str`），避免 dashboard 反向依赖 mcpgw。

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
    pub about: AboutInfo,
}
```
handler 的**只读共享态**（装配期一次性注入）：活快照句柄 + 实时指标 sink + 可选 discovery ring +
可选 per-call ring（`calls`，仅 dashboard 启用时 `Some`）+ 配置上游列表/策略名 + 历史 JSONL 路径 + 启动时刻 +
启动时组装的只读 `about`（`AboutInfo`，非敏感，见 [`about.rs`](#aboutrsaboutsettings-只读视图启动时组装仅非敏感)）。

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
| `call_filter_from_query` | `(&HashMap<String,String>) -> CallFilter` | 从查询参数 `meta`/`upstream`/`tool`（→`target_tool`）/`outcome`/`since`/`until`/`q`/`arg_key`/`arg_val` 构造过滤器（`since`/`until` 解析为 `u64` ms；`q`/`arg_key`/`arg_val` 原样 clone，内容过滤仅 live 生效） |
| `calls` | `(&AppState, &CallFilter, source, scan_limit, limit, offset) -> CallsResponse` | `source=="history"` → `replay_audit_calls(audit_path, scan_limit)`（无 path→`history_unavailable`、`total`=全部命中、`skip(offset).take(limit)`）；否则 `calls.query(filter, limit, offset)`（未启用→空、`history_unavailable=false`） |
| `call_detail` | `(&AppState, id) -> Option<CallItem>` | `is_history_id(id)` → 重扫 `CALL_HISTORY_SCAN` 行回放后按 id 定位；否则按十进制 live seq 取环 `get(seq)`；找不到/源不可用→`None` |
| `is_history_id` | `(id: &str) -> bool` | id 以 `h` 开头即历史（`"h{ts}-{n}"`）；否则按 live ring 十进制 seq。格式判定集中于此，使 handler 的 blocking-pool 决策与 `call_detail`/`trace_detail` 源路由不漂移 |
| `parse_window` | `(&HashMap<String,String>) -> u64` | 解析查询参数 `window`（ms）：缺省 `900_000`（15min），`clamp(60_000, 86_400_000)`（[1min, 24h]）；解析失败回退缺省 |
| `activity` | `(&AppState, window_ms) -> activity::ActivityResponse` | dashboard 启用且 `calls` 为 `Some` → `ring.activity(window_ms)`（聚合 live 环）；否则 `activity::aggregate(&[], window_ms, now)`（空：24 个全 0 桶）。**仅元数据** |

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
| GET | `/api/calls?source=live\|history&meta=&upstream=&tool=&outcome=&since=&until=&q=&arg_key=&arg_val=&limit=&offset=` | `Json<CallsResponse>`（`limit` 缺省 100、`min(MAX_HISTORY_LIMIT=50_000)`；`offset` 缺省 0；`source` 缺省 `"live"`；`q`/`arg_key`+`arg_val` 内容过滤**仅 live**，history 自动忽略；列表永不含内容；history 路径走 `spawn_blocking`） |
| GET | `/api/calls/{id}` | `Json<CallItem>` 或 404（`h…`→历史回放定位；否则按 live seq 取环） |
| GET | `/api/activity?window=<ms>` | `Json<ActivityResponse>`（`window` 缺省 900_000=15min、`clamp(60_000, 86_400_000)`=[1min,24h]；聚合 live 环、**仅元数据**、固定 24 桶、各 Top-5；live 内存读，无 `spawn_blocking`） |
| GET | `/api/about` | `Json<AboutInfo>`（`h_about` 直接 clone 序列化启动时组装好的 `state.about`，**运行期不变、零计算**、无 `spawn_blocking`；仅非敏感配置/限额 + 版本，绝不含密钥/env，见 [`about.rs`](#aboutrsaboutsettings-只读视图启动时组装仅非敏感)） |
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
- **Activity sparkline 交互**（纯前端、后端无新增）：`Sparkline.svelte` 渲染 24 根 flex 柱，props
  `buckets`/`bucketMs`/`onpick(since, until)`——非零柱是 `<button>`，点击回调绝对窗 `onpick(b.t, b.t + bucketMs - 1)`
  （闭区间），零柱是非交互细基线。`Activity.svelte` 新增 `onpick` prop 并透传给 `Sparkline`。`bucketSel.svelte.js`
  导出 `pendingBucket`（`$state`）作 Overview 点柱跳 `#/calls` 的跨页暂存窗，Calls 组件初始化时消费一次后清空。Calls
  的 `bucketSel`（`{since, until}`）与滚动时间范围 `rangeMs` **互斥**：`query` derived **无条件读** `bucketSel`（避免
  null→设的条件依赖陷阱）并在有桶选时附 `since`/`until`，`loadCalls` 的滚动 `since` 仅在**无 `bucketSel`** 时附加；
  `setRange`/时间范围 chip 清桶选，selected-bucket chip 也清桶选。**后端无新增**——复用既有 `/api/calls` 的
  `since`/`until`（闭区间 `[since, until]`），`/api/activity` 不变。
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
