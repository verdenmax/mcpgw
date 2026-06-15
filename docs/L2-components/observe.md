# L2 — `observe` 组件

## 职责

网关的**调用观测层**（M6.T1）：定义**结构化、仅元数据**的元工具调用记录 `CallRecord`、扇出用的
`CallSink` trait，以及把记录写成结构化 `tracing` 事件的 `TracingSink`。它是「在调用边界构造一条记录 →
分发给每个配置的 sink」这一**无存储、无 HTTP** 接缝的**单一来源**：T1（tracing）与 T3（审计 JSONL）
共用之。

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

### testkit `CaptureSink`
`testkit` feature 下导出的测试 sink：把每条记录克隆进内部 buffer，`records()` 取快照供断言。

详见 L4：`docs/L4-api/observe-lib.md`（逐项签名、序列化 key 集合锁死、`now_unix_ms` 语义、扩展点）。

## 依赖

- `serde` + `serde_json`（`CallRecord`/枚举序列化）。
- `tracing`（`TracingSink` 的结构化事件）。
- dev：`tokio`（`rt` + `macros`）——仅供测试。
- **无 `reqwest`/HTTP、无存储、无 tokio 运行时依赖**；**不依赖任何兄弟 crate**（`metatools` 也**不**反向
  依赖 `observe`，保持元工具逻辑纯净）。

## 被谁使用

- `downstream`（`GatewayServer::call_tool`）：每次元工具调用计时、分类 `error_kind`、构造一条
  `CallRecord`，再扇出到注入的 `sinks`。
- `mcpgw`（bin）的 `serve`：装配**默认 `[TracingSink]`** 这一 sinks 切片，注入 stdio
  （`GatewayServer::new`）与 http（`build_router`）两个传输（共享同一切片）。

## 不负责

- **审计持久化**（把记录落盘/入库）——属 M6.T3 的 `JsonlSink`（实现同一 `CallSink` trait 接入）。
- **用量指标/聚合与导出**（Prometheus/OTel）——属 M6.T2 的 `MetricsSink`。
- 计时/分类/构造记录本身——发生在 `downstream`，本 crate 只定义**记录形状与 sink 契约**。

## 向下导航

- 逐文件 API 见 L4：[observe-lib](../L4-api/observe-lib.md)
- 埋点位置/延迟基准/`error_kind` 分类见 L3：[downstream](../L3-details/downstream.md)
- 装配入口见：[mcpgw-cli L2](./mcpgw-cli.md) · [downstream L2](./downstream.md)
