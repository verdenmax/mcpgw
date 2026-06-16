# L3 — `dashboard` 细节

只读可视化面板（子系统 A）的进程模型、数据来源、隐私边界，以及 `MetricsSink` 桶/分位算法、
`DiscoveryRingSink` ring+writer、history 限量回放的实现细节。

## 进程模型：独立任务 + 独立端口 + panic 隔离 + 优雅关停

- **独立端口、独立 task**：面板是 `mcpgw serve` 里一个**与下游 stdio/HTTP 完全分开**的 axum server，
  跑在自己的 `[dashboard].bind`（默认 `127.0.0.1:8971`）上、作为一个 `tokio::spawn` 的 task，带
  `with_graceful_shutdown(oneshot)`。它**只读** `Arc<GatewayState>` 与两个 sink，不参与 MCP 协议、不改状态。
- **localhost、无鉴权**：默认绑 loopback、**不挂任何 auth 层**（与下游 HTTP 的 Bearer 鉴权不同）。装配期
  若检测到「无 auth 且绑非 loopback」会 `warn`（复用 `unauthenticated_public_bind`，把面板按「无 key」判定），
  但**不**拒绝启动——外网暴露应交给反向代理。
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
| `/api/tools?q=` | 活快照 `catalog()`，可选子串过滤（对 `qualified_name` 与 `description` 做小写 `contains`） |
| `/api/metrics` | `MetricsSink::snapshot()`（**实时**聚合） |
| `/api/traces?source=live` | `DiscoveryRingSink::recent(limit)`（内存 ring；未启用 discovery → 空） |
| `/api/traces?source=history` | `replay_discovery(discovery_path, limit)`（discovery JSONL 回放；无 path → `history_unavailable`） |
| `/api/metrics/history` | `replay_audit_metrics(audit_path, limit, bucket_ms)`（审计 JSONL 回放；无 path → `history_unavailable`） |

即面板把**四类来源**拼起来：① 活快照（catalog + last_summary）、② 实时 `MetricsSink`、③ 实时
`DiscoveryRingSink`、④ 历史 JSONL 回放（审计 + discovery）。**实时与历史是两条独立来源**：实时来自内存
sink，历史来自落盘 JSONL，互不依赖（任一缺失只让对应视图降级，不影响其它）。

## 隐私边界（与调用观测隔离）

- 调用观测的 `observe::CallRecord` 与审计 JSONL **仍是仅元数据**——只有 size、分类、上游名等，**永不含**
  query 文本/参数/结果内容。面板的 `overview`/`upstreams`/`metrics`/`metrics-history` 视图只读这些元数据。
- **query 文本 + 命中工具名 + 分数**走**另一条独立通道** `observe::DiscoveryRecord`（经 `DiscoverySink`），
  且**默认关闭**：仅当 `[dashboard].trace_queries = true` 时才构造 `DiscoveryRingSink` 并注入下游，下游的
  `search_tools` 分支才扇出 `DiscoveryRecord`。该通道与仅元数据的 `CallSink` 物理隔离，绝不让 query 漏进
  tracing/审计。`/api/traces` 是唯一会回显 query/工具名的端点。
- SPA 渲染时对**所有不可信字段**（query、工具名、上游名、skip 原因、transport、meta_tool 名）一律
  `escapeHtml` 后再写入 `innerHTML`，避免上游/客户端控制的字符串造成 stored XSS（crate 单测锁死这五处转义）。

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

- **ring**：`VecDeque<DiscoveryRecord>`，容量 `cap.max(1)`；`record` 满则 `pop_front` 再 `push_back`，
  `recent(limit)` 用 `iter().rev().take(limit)` 给出 newest-first。读写都在短临界区内的 `Mutex`。
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
- **坏行跳过**：`replay_discovery` 对每行 `serde_json::from_str` 失败即 `filter_map` 跳过；
  `replay_audit_metrics` 只解析 `{ ts_unix_ms, outcome }` 两字段，坏行跳过。
- **方向**：`replay_discovery` 末尾 `reverse()` 给 newest-first（与实时 ring 一致）；`replay_audit_metrics`
  按 `BTreeMap` 桶键升序，oldest-first（适合画时间序列）。桶起点 `ts - (ts % bucket_ms)`，`bucket_ms` 至少 1。

## 测试覆盖

- `metrics.rs`：聚合计数/错误/延迟、分位单调且 ≤max、`Timeout` 记为 error、`per_upstream` 封顶（总量仍全计）、
  空快照清零。
- `trace.rs`：ring 封顶 + newest-first、`recent` 限量、file writer 持久化并在 `join` 时 drain。
- `history.rs`：缺文件不可用、`replay_discovery` 跳坏行且 newest-first、`replay_audit_metrics` 分桶计数、
  `Timeout` 记为 error。
- `api.rs`：`overview` 报策略/上游数、`upstreams` 重建前 `unknown`、`metrics` 反映已记录调用、缺 path 时
  history 不可用、空 catalog 的 `tools` 过滤。
- `lib.rs`：内嵌资源就位且接线、SPA 五处不可信字段均经 `escapeHtml`。
- `crates/mcpgw/tests/dashboard.rs`（**默认 `#[ignore]`**，绑端口，`--ignored` 跑）：`serve` 起面板、
  `/api/overview` 报 `strategy=bm25`、一次 `search_tools` 被 `/api/traces?source=live` 捕获到 query。

## 相关

- 接口见 L2：[dashboard](../L2-components/dashboard.md)；逐文件 API 见 L4：[dashboard](../L4-api/dashboard.md)
- 接缝来源见：[observe-lib L4](../L4-api/observe-lib.md)（`DiscoverySink`/`DiscoveryRecord`）·
  [downstream L3](./downstream.md)（`search_tools` 捕获 + `upstream` 归因安全修复）
- 装配/关停顺序见：[mcpgw-main L4](../L4-api/mcpgw-main.md)；配置见：[config L3](./config.md)（`[dashboard]` 段）
