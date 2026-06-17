# L2 — `dashboard` 组件

## 职责

网关的**只读可视化面板**（子系统 A）：把 `gateway` 的活快照、`observe` 的调用观测与可选的历史 JSONL
回放聚合起来，经一个**独立 localhost 端口**上的小 axum server 暴露为 8 个 `/api/*` JSON 端点 + 一个
**Svelte 5 + Vite 构建、经 `rust-embed` 内嵌**的 SPA（`assets::static_handler` fallback 交付）。它**只读、不改动
任何网关状态**，默认关闭、须显式 opt-in。

本 crate 提供三个接入 `observe` 接缝的 sink：
- `MetricsSink` 实现 `observe::CallSink`，**实时聚合**每个元工具的调用数/错误数/延迟分位（p50/p95/max）
  与每个上游的调用/错误数；
- `CallRingSink` 实现 `observe::CallSink`，把**逐条** `CallRecord`（仅元数据）存进**有界环形缓冲**
  （newest-first，上界 `[dashboard].call_buffer`），支撑 Calls 下钻的列表/详情；
- `DiscoveryRingSink` 实现 `observe::DiscoverySink`，把 `search_tools` 的 `query → 命中工具+分数`
  追踪存进**有界环形缓冲**（newest-first），并可选地经后台 writer 线程落一份 discovery JSONL 供历史回放。

**隐私边界**：调用观测的 `CallRecord`/审计 JSONL 仍是**仅元数据**（无 query/参数/结果内容），面板的
**指标**视图只读这些元数据；**query 文本 + 命中工具名**是**另一条独立、opt-in（`[dashboard].trace_queries`）
的 discovery 通道**，绝不混入仅元数据的调用 sink。

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
| `DiscoveryRingSink` | impl `observe::DiscoverySink` | 最近 discovery 追踪的有界环形缓冲（容量 `cap.max(1)`，读 newest-first）；有 path 时再经后台线程把每条记录 append 成一行 discovery JSONL |
| `DiscoveryRingSink::spawn` | `(cap: usize, path: Option<&Path>) -> io::Result<(Self, Option<DiscoveryWriter>)>` | 建 ring；`Some(path)` 时打开文件（create+append）+ 起命名 `discovery-writer` 的 OS 线程，返回其 `DiscoveryWriter`（关停 drain 用）。打不开/起不来即 `Err` |
| `DiscoveryRingSink::recent` | `(&self, limit: usize) -> Vec<DiscoveryRecord>` | newest-first 最多 `limit` 条 |
| `DiscoveryRingSink::dropped_count` | `(&self) -> u64` | 因 writer channel 满而丢弃的条数（测试/诊断） |
| `DiscoveryWriter::join` | `(self)` | 阻塞至 writer 线程 drain+flush+fsync 退出（须先 drop 所有 sink clone 关闭 channel） |

### 逐条调用环 `CallItem` / `CallFilter` / `CallRingSink`（`calls.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `CallRingSink` | impl `observe::CallSink` | 逐条 `CallRecord` 的有界环（满淘汰最旧），每条插入在锁内分配单调 `seq` 作 live id；容量 `[dashboard].call_buffer` |
| `CallRingSink::new` | `(cap: usize) -> Self`（`cap.max(1)`） | 空环 |
| `CallRingSink::query` | `(&CallFilter, limit, offset) -> (Vec<CallItem>, usize)` | newest-first 一页 + `total`（计全部命中，独立于分页） |
| `CallRingSink::get` | `(&self, seq: u64) -> Option<CallItem>` | 按 live seq 取单条 |
| `CallItem` | `Serialize` | live 环与 history 回放共用的 owned 项：`id` / `ts_unix_ms` / `meta_tool` / `target_tool?` / `upstream?` / `latency_ms` / `outcome` / `error_kind?` / `arg_bytes` / `result_bytes`。仅元数据 |
| `CallFilter` | `Default` | `meta_tool`/`upstream`/`target_tool`/`outcome`/`since_ms`/`until_ms`（均 `Option`，`None`=全匹配；时间为闭区间）；`matches(&CallItem)` 统一过滤 live 与 history |

### 历史回放 `replay_audit_metrics` / `replay_discovery` / `replay_audit_calls` / `MetricBucket`（`history.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `replay_discovery` | `(path: &Path, limit: usize) -> (Vec<DiscoveryRecord>, bool)` | 回放 discovery JSONL，扫**最后** `limit` 行、newest-first，坏行跳过；`bool` = 文件可读 |
| `replay_audit_metrics` | `(path: &Path, limit: usize, bucket_ms: u64) -> (Vec<MetricBucket>, bool)` | 回放审计 JSONL 进定宽时间桶（oldest-first），非 `"ok"` outcome 记为 error（与实时 `MetricsSink` 一致） |
| `replay_audit_calls` | `(path: &Path, scan_limit: usize, filter: &CallFilter) -> (Vec<CallItem>, bool)` | 回放审计 JSONL 为逐条 `CallItem`，newest-first，稳定 id `"h{ts}-{n}"`，`filter` 在 id 分配后应用；`bool` = 文件可读 |
| `MetricBucket` | `Serialize` | `bucket_start_ms` / `calls` / `errors` |

### API 状态与路由 `AppState` / `UpstreamInfo` / `build_dashboard_router`（`api.rs` / `lib.rs`）

| 项 | 签名 | 说明 |
|----|------|------|
| `AppState` | `Clone` | 面板 handler 的只读共享态：`gateway` / `metrics` / 可选 `discovery` ring / 可选 `calls`（逐条调用环，仅 dashboard 启用时 `Some`）/ `upstreams: Vec<UpstreamInfo>` / `strategy` / 可选 `audit_path` / `discovery_path` / `started_at` |
| `UpstreamInfo` | `Serialize` | 一个配置上游的静态身份：`name` / `transport`（装配期由 `Config` 给出） |
| `build_dashboard_router` | `(state: Arc<AppState>, enforce_loopback_host: bool) -> axum::Router` | 装配 8 个 `/api/*` 路由 + `assets::static_handler` fallback（内嵌 SPA：`/` → `index.html`、`/assets/*` → 内嵌资源），`with_state(state)`；`enforce_loopback_host` 时挂反 DNS-rebinding 的 Host 校验层 |

`/api/*` 端点（逐符号见 L4）：`/api/overview`、`/api/upstreams`、`/api/tools?q=`、`/api/metrics`、
`/api/traces?source=live|history&limit=`、`/api/metrics/history?limit=&bucket_ms=`、
`/api/calls?source=live|history&meta=&upstream=&tool=&outcome=&since=&until=&limit=&offset=`、`/api/calls/{id}`。

## 依赖

- 内部：`gateway`（`GatewayState`：读活快照 + `last_summary`）、`observe`（`CallSink`/`CallRecord` 与
  `DiscoverySink`/`DiscoveryRecord` 契约）、`catalog`（经 `GatewaySnapshot::catalog()` 列工具）、`config`
  （装配期取上游/策略/路径）。
- 外部：`axum`（router/handler）、`tokio`（serve）、`serde`/`serde_json`（视图序列化、JSONL 读写）、`tracing`、
  `rust-embed`（编译期内嵌 `ui/dist/` 静态产物，`debug-embed`+`mime-guess`）。
- 前端工程在 `crates/dashboard/ui/`（Svelte 5 + Vite）：`npm run build` 重新生成 `ui/dist/`（**已入库**，故 `cargo build`
  不依赖 node；`node_modules/` gitignore），由 `assets.rs` 经 `rust-embed` 内嵌。
- discovery JSONL writer **只用 `std::thread` + `std::sync::mpsc`（有界 `sync_channel`）+ `std::fs`**（与
  `observe` 的审计 writer 同构），落盘不进 tokio 运行时。

## 被谁使用

- `mcpgw`（bin）的 `serve`：`[dashboard].enabled` 时构造 `MetricsSink` 与 `CallRingSink`（均加进 `CallSink`
  切片）、按 `trace_queries` 构造 `DiscoveryRingSink`（注入 stdio + HTTP 两个下游的 `DiscoverySink` 切片），
  并把面板起为**自己端口上的独立 task**（默认 `127.0.0.1:8971`，localhost、无鉴权），带优雅关停与有界 writer drain。
  详见 L4 [mcpgw-main](../L4-api/mcpgw-main.md)。

## 不负责

- **任何写操作 / 控制面**：面板纯只读，不暴露重启上游、改配置、撤 key 等动作。
- **鉴权 / TLS / 反代**：默认绑 localhost、无 auth；非 loopback 绑定只 `warn`，不内建鉴权（留给反代）。
- **图表库 / SSE / WebSocket**：SPA 用 **Svelte 5 + Vite** 构建、产物经 `rust-embed` 内嵌；仍每 3s 轮询
  `/api/*`、**无 SSE/WS**，也无图表库。
- **指标导出（Prometheus/OTel）**：属 `observe` 接缝的另一类 sink（M6.T2），不在本 crate。

## 向下导航

- 逐文件 API 见 L4：[dashboard](../L4-api/dashboard.md)
- 进程模型 / 算法 / 数据来源 / 隐私见 L3：[dashboard](../L3-details/dashboard.md)
- 接缝来源见：[observe L2](./observe.md) · [observe-lib L4](../L4-api/observe-lib.md) ·
  [downstream L2](./downstream.md)
- 装配入口见：[mcpgw-cli L2](./mcpgw-cli.md) · [mcpgw-main L4](../L4-api/mcpgw-main.md)
