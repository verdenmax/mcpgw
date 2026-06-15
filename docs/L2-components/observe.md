# L2 — `observe` 组件

## 职责

网关的**调用观测层**（M6.T1/T3）：定义**结构化、仅元数据**的元工具调用记录 `CallRecord`、扇出用的
`CallSink` trait、把记录写成结构化 `tracing` 事件的 `TracingSink`，以及**可选的 JSONL 审计落盘**
（`JsonlSink` + 专用 OS 线程 writer，**std-only**）。它是「在调用边界构造一条记录 → 分发给每个配置的
sink」这一接缝的**单一来源**：T1（tracing）与 T3（审计 JSONL）共用之。

本 crate **刻意保持极小且无副作用**：`CallRecord` **按类型构造即不可能携带参数/结果内容**——只有 size
（`arg_bytes`/`result_bytes`），因此观测**永远不会把 secret/PII 泄进日志或审计轨**。真正的埋点（计时、
分类、构造记录、扇出）发生在 `downstream::GatewayServer::call_tool`；`metatools` crate 保持**纯函数、不
依赖 `observe`**。

## 公开接口

### 类型 `CallRecord`
一次元工具调用的**仅元数据**记录。

| 字段 | 类型 | 说明 |
|------|------|------|
| `ts_unix_ms` | `u64` | 记录构造时刻（unix 毫秒，见 `now_unix_ms`） |
| `meta_tool` | `MetaTool` | 哪个元工具（`search_tools`/`get_tool_details`/`call_tool`，snake_case 序列化） |
| `target_tool` | `Option<String>` | `call_tool` 的目标工具 qualified name；其它元工具 `None`（不序列化） |
| `upstream` | `Option<String>` | 由 `target_tool` 的 `"__"` 前缀取出的上游 server 名；`None` 不序列化 |
| `latency_ms` | `u64` | 分派耗时（毫秒，只覆盖调用本身） |
| `outcome` | `CallOutcome` | `ok`/`error`/`timeout` |
| `error_kind` | `Option<&'static str>` | 失败分类（稳定字符串）；成功 `None`（不序列化） |
| `arg_bytes` / `result_bytes` | `usize` | 入参/结果 JSON 的**字节数**（仅 size，**无内容**） |

### 枚举 `MetaTool` / `CallOutcome`
都 `#[serde(rename_all = "snake_case")]`，且各带 `as_str() -> &'static str`（与 serde 串**逐字一致**），
使 tracing 字段与 serde 输出用同一拼写。

### trait `CallSink`

| 项 | 签名 | 说明 |
|------|------|------|
| `record` | `(&self, rec: &CallRecord)` | 接收一条记录。**契约：非阻塞、绝不 panic**（观测失败绝不影响调用本身） |

`Send + Sync`，可放进跨线程共享的 `Arc<[Arc<dyn CallSink>]>`。

### 类型 `TracingSink`
实现 `CallSink`：把每条记录发为结构化 `tracing::info!(..., "tool_call")` 事件（复用进程 subscriber，
只发元数据）。

### 类型 `JsonlSink` / `spawn_writer` / `AuditWriter` / `AUDIT_CHANNEL_CAPACITY`（M6.T3，可选审计落盘）
**可选的 JSONL 审计 sink**，std-only（无 tokio）。

| 项 | 签名 | 说明 |
|----|------|------|
| `JsonlSink` | impl `CallSink`（`#[derive(Clone)]`） | `record()` = `serde_json::to_string(rec)` + `try_send` 进有界 channel；满/断连 → `dropped` 计数 + **首次及每个 2 的幂次**限频 warn；调用热路径**绝不阻塞**。clone 共享同一 sender 与同一计数 |
| `JsonlSink::dropped_count` | `(&self) -> u64` | 至今被丢弃的记录数 |
| `spawn_writer` | `(path: &Path, capacity: usize) -> io::Result<(JsonlSink, AuditWriter)>` | 打开文件（create+append）+ 起命名为 `audit-writer` 的 OS 线程跑 writer loop；文件打不开/线程起不来 → `Err`（调用方 fail-fast） |
| `AuditWriter` | 持 writer 线程 `JoinHandle`（**不持 sender**） | `join(self)` 阻塞至 drain+flush+fsync+退出——只在**所有 `JsonlSink` clone drop**（channel 断连）后发生 |
| `AUDIT_CHANNEL_CAPACITY` | `pub const usize = 1024` | sink→writer 有界 channel 容量（满则丢弃，不阻塞） |

writer loop：批量 drain + 每批 flush 到 OS；干净断连时**最终 flush + `sync_all`（fsync）一次**再退出；写失败
**只限频 warn、不退出**（瞬时故障自愈）。**只序列化 `CallRecord`** → 审计行**永不含 payload**（仅元数据不变量）。

### testkit `CaptureSink`
`testkit` feature 下导出的测试 sink：把每条记录克隆进内部 buffer，`records()` 取快照供断言。

详见 L4：`docs/L4-api/observe-lib.md`（逐项签名、序列化 key 集合锁死、`now_unix_ms` 语义、扩展点）；
审计 sink 逐项见 `docs/L4-api/observe-audit.md`（`JsonlSink`/`spawn_writer`/`AuditWriter`、writer loop 语义）。

## 依赖

- `serde` + `serde_json`（`CallRecord`/枚举序列化）。
- `tracing`（`TracingSink` 的结构化事件、审计的限频 warn）。
- **审计落盘只用 `std::thread`（专用 writer 线程）+ `std::sync::mpsc`（`sync_channel` 有界队列）+
  `std::fs`/`BufWriter`（append + flush + `sync_all` fsync）**，**不引入 `tokio`**。
- **无 `reqwest`/HTTP、无第三方存储、无 tokio 运行时依赖**；测试均为同步 `#[test]`，故连 dev-dependency 也不需要 `tokio`。
  **不依赖任何兄弟 crate**（`metatools` 也**不**反向
  依赖 `observe`，保持元工具逻辑纯净）。

## 被谁使用

- `downstream`（`GatewayServer::call_tool`）：每次元工具调用计时、分类 `error_kind`、构造一条
  `CallRecord`，再扇出到注入的 `sinks`。
- `mcpgw`（bin）的 `serve`：装配以 `TracingSink` 打底的 sinks 切片，注入 stdio
  （`GatewayServer::new`）与 http（`build_router`）两个传输（共享同一切片）。
- `mcpgw serve` 还据 `[audit]` 段装配 `JsonlSink`（`enabled` 时 `spawn_writer` 打开文件 fail-fast、追加进
  sinks 切片，并持 `AuditWriter` 在关停时有界优雅 drain）。

## 不负责

- **用量指标/聚合与导出**（Prometheus/OTel）——属 M6.T2 的 `MetricsSink`（实现同一 `CallSink` trait 接入）。
- 计时/分类/构造记录本身——发生在 `downstream`，本 crate 只定义**记录形状、sink 契约与可选的 JSONL 落盘**。
- 审计文件的**轮转/重开**——`JsonlSink` 单纯 append，无内建 rotation/SIGHUP 重开，须由外部
  logrotate 等处理（见 [config L3](../L3-details/config.md) 的 `[audit]` 运维说明）。

## 向下导航

- 逐文件 API 见 L4：[observe-lib](../L4-api/observe-lib.md) · [observe-audit](../L4-api/observe-audit.md)
- 埋点位置/延迟基准/`error_kind` 分类见 L3：[downstream](../L3-details/downstream.md)
- 装配入口见：[mcpgw-cli L2](./mcpgw-cli.md) · [downstream L2](./downstream.md)
