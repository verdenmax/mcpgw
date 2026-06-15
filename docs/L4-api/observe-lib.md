# L4 — `crates/observe/src/lib.rs` API

源文件：`crates/observe/src/lib.rs`。**结构化、仅元数据**的网关元工具调用观测：定义 `CallRecord`
（**不**含任何参数/结果载荷，只含 size）、`CallSink` trait 与 `TracingSink`。这是 M6.T1 落地的
「在调用边界**构造一条记录 → 扇出到每个配置的 sink**」的**本文件自身无存储、无 HTTP** 共享接缝：T1（tracing）与
T3（审计 JSONL）共用它——一条记录在 `downstream::GatewayServer::call_tool` 里构造一次，再被分发给
每个 sink（M6.T3 的可选 JSONL 落盘 `JsonlSink` 在同 crate 的 `audit.rs`，见 [observe-audit](./observe-audit.md)）。

## `enum MetaTool`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaTool { SearchTools, GetToolDetails, CallTool }
```
被调用的元工具。**serde 序列化为 snake_case** 短字符串（`"search_tools"` / `"get_tool_details"` /
`"call_tool"`）。

### `MetaTool::as_str`
```rust
pub fn as_str(&self) -> &'static str
```
返回**与 serde 表示完全一致**的 snake_case token。这保证 `TracingSink` 的 tracing 字段串与（未来的）
JSONL sink 的 serde 串**用同一拼写**描述同一条记录。

## `enum CallOutcome`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome { Ok, Error, Timeout }
```
一次元工具调用的结局。serde 序列化为 `"ok"` / `"error"` / `"timeout"`。

### `CallOutcome::as_str`
```rust
pub fn as_str(&self) -> &'static str
```
返回与 serde 表示一致的 snake_case token（同上，两类 sink 拼写一致）。

## `struct CallRecord`
```rust
#[derive(Debug, Clone, Serialize)]
pub struct CallRecord {
    pub ts_unix_ms: u64,
    pub meta_tool: MetaTool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub latency_ms: u64,
    pub outcome: CallOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
    pub arg_bytes: usize,
    pub result_bytes: usize,
}
```
一次元工具调用的**仅元数据**记录。**按类型构造即不可能携带参数/结果内容**——只有 size
（`arg_bytes` / `result_bytes`），因此它**永远不会把 secret/PII 泄进日志或审计轨**。

| 字段 | 类型 | 含义 |
|------|------|------|
| `ts_unix_ms` | `u64` | 记录构造时的 unix 毫秒时间戳（见 `now_unix_ms`） |
| `meta_tool` | `MetaTool` | 哪个元工具被调用（snake_case 序列化） |
| `target_tool` | `Option<String>` | `call_tool` 转发的**目标工具 qualified name**（如 `github__create_issue`）；其它元工具为 `None`，**None 不序列化** |
| `upstream` | `Option<String>` | 由 `target_tool` 的 `split_once("__")` 取出的**上游 server 前缀**（如 `github`）；无目标时 `None`，**None 不序列化** |
| `latency_ms` | `u64` | 分派耗时（毫秒）——只覆盖调用本身，不含记账开销（见下游 L3） |
| `outcome` | `CallOutcome` | `ok` / `error` / `timeout` |
| `error_kind` | `Option<&'static str>` | 失败时的**稳定字符串分类**（如 `timeout`/`tool_not_found`，由下游 `classify` + 内联臂给出）；成功为 `None`，**None 不序列化** |
| `arg_bytes` | `usize` | 入参 JSON 的**字节数**（仅 size，**无内容**） |
| `result_bytes` | `usize` | 结果 JSON 的**字节数**（仅 size，**无内容**；`Err` 路径为 0） |

**仅元数据不变量**：三个 `Option` 字段用 `#[serde(skip_serializing_if = "Option::is_none")]`，故
`None` 时不出现在输出里。crate 自带单测把序列化后的 key 集合**锁死**为恰好这 9 个元数据键（断言里
显式排除 `arguments`/`args`/`result`/`content`/`text` 等载荷键），任何新增字段都必须在该测试里被
有意识地承认。

### `CallRecord::now_unix_ms`
```rust
pub fn now_unix_ms() -> u64
```
当前 unix 毫秒时间（供 `ts_unix_ms`）：`SystemTime::now().duration_since(UNIX_EPOCH)`，时钟早于纪元
等异常时**回退 0**（不 panic）。

## `trait CallSink`
```rust
pub trait CallSink: Send + Sync {
    fn record(&self, rec: &CallRecord);
}
```
调用观测的 sink。**契约**：实现**必须非阻塞、且绝不 panic**——一次观测失败**绝不能**影响工具调用
本身（下游在每次调用尾部 `for sink in self.sinks.iter() { sink.record(&rec); }` 同步扇出）。
`Send + Sync` 使其可放进跨线程共享的 `Arc<[Arc<dyn CallSink>]>`。

## `struct TracingSink`
```rust
pub struct TracingSink;
impl CallSink for TracingSink { fn record(&self, r: &CallRecord) { ... } }
```
**T1 sink**：把每条记录作为一条**结构化 `tracing` 事件**发出（复用进程已装好的 subscriber，下游里是
`mcpgw serve` 的 stderr fmt subscriber）：
```rust
tracing::info!(
    meta_tool = r.meta_tool.as_str(),
    target_tool = r.target_tool.as_deref(),
    upstream = r.upstream.as_deref(),
    latency_ms = r.latency_ms,
    outcome = r.outcome.as_str(),
    error_kind = r.error_kind,
    arg_bytes = r.arg_bytes,
    result_bytes = r.result_bytes,
    "tool_call"
);
```
事件名为 `"tool_call"`；枚举字段用 `as_str()`，与 serde 串一字不差。`Option` 字段经 `as_deref()` /
原值传入（`None` 渲染为缺省）。**只发元数据**，无参数/结果内容。

## testkit：`struct CaptureSink`
```rust
#[cfg(feature = "testkit")]
pub use capture::CaptureSink;
```
仅在 `testkit` feature 下导出的**测试 sink**：把每条 `record` 克隆进内部 `Arc<Mutex<Vec<CallRecord>>>`
供断言。

| 项 | 签名 | 说明 |
|----|------|------|
| `new` | `() -> Self` | 等价 `Default::default()` |
| `records` | `(&self) -> Vec<CallRecord>` | 至今所见全部记录的快照（克隆） |
| `record` | impl `CallSink` | 把记录推进内部 buffer |

`#[derive(Clone)]`（克隆共享同一份 `Arc` buffer），故可 `cap.clone()` 装进 sinks、再从原句柄读
`records()`。下游 e2e 测试用它验证「每次网关元工具调用恰好一条记录、未知元工具名不记录」。

## 依赖与扩展点

- 依赖：`serde` + `serde_json`（序列化）、`tracing`（`TracingSink`）。本文件 `lib.rs` **无 HTTP、无存储、无 tokio**
  （测试均为同步 `#[test]`，亦无 `tokio` dev-dependency）；M6.T3 的可选 JSONL 落盘在同 crate 的 `audit.rs`，
  **仅用 std 文件 I/O + 专用 writer 线程，仍不引入 tokio/HTTP**（见 [observe-audit](./observe-audit.md)）。
- **扩展点**：这是 T1/T3 共享的「instrument → multi-sink」接缝，已被 **`JsonlSink`（M6.T3 审计落盘）** 复用；
  未来的 **`MetricsSink`（M6.T2 用量指标）** 同样实现同一个 `CallSink` trait、被加进同一个 sinks 切片即可接入，
  无需改下游的构造/扇出逻辑。

## 测试
- `crates/observe/src/lib.rs` 单测：枚举序列化为 snake_case 短串且 `as_str()` 与之一致；序列化 key 集合
  **恰好**是 9 个元数据键（锁死「无载荷」不变量）；`None` Optional 不序列化。
- `crates/observe/tests/capture.rs`（需 `testkit`）：`CaptureSink` 按顺序记录多条 `CallRecord`。

> 谁产生记录、如何分类 `error_kind`、延迟测量基准见 L3：[downstream](../L3-details/downstream.md)；
> 组件视角见 L2：[observe](../L2-components/observe.md)。
