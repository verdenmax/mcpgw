# L4 — `crates/observe/src/lib.rs` API

源文件：`crates/observe/src/lib.rs`（+ 由它 re-export 的 `crates/observe/src/discovery.rs`）。
**结构化、仅元数据**的网关元工具调用观测：定义 `CallRecord`（**不**含任何参数/结果载荷，只含 size）、
`CallSink` trait 与 `TracingSink`。这是 M6.T1 落地的「在调用边界**构造一条记录 → 扇出到每个配置的 sink**」的
**本文件自身无存储、无 HTTP** 共享接缝：T1（tracing）与 T3（审计 JSONL）共用它——一条记录在
`downstream::GatewayServer::call_tool` 里构造一次，再被分发给每个 sink（M6.T3 的可选 JSONL 落盘 `JsonlSink`
在同 crate 的 `audit.rs`，见 [observe-audit](./observe-audit.md)）。

本文件还定义 `CallContent` + `CallContentSink`：一条**与仅元数据 `CallRecord` 物理隔离**的调用**内容**
（args/result 文本）扇出契约，使参数/结果内容**绝不**经 tracing/审计 sink 流出，只入 dashboard 的内存环
（见下文「调用内容契约」）。

子系统 A（dashboard）另起一条**与仅元数据 `CallRecord` 隔离、opt-in** 的发现追踪通道：`DiscoveryRecord` /
`DiscoveryHit` / `DiscoverySink`（`discovery.rs`，经 `lib.rs` re-export，见文末「发现追踪契约」）。

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
| `upstream` | `Option<String>` | 由下游按 `target_tool` 在**工具目录里解析**出的真实上游 `server` 名（如 `github`）；无目标或目标在目录里查不到时 `None`，**None 不序列化**。**不再**靠 `split_once("__")` 切 client 提供的名字（见安全说明） |
| `latency_ms` | `u64` | 分派耗时（毫秒）——只覆盖调用本身，不含记账开销（见下游 L3） |
| `outcome` | `CallOutcome` | `ok` / `error` / `timeout` |
| `error_kind` | `Option<&'static str>` | 失败时的**稳定字符串分类**（如 `timeout`/`tool_not_found`，由下游 `classify` + 内联臂给出）；成功为 `None`，**None 不序列化** |
| `arg_bytes` | `usize` | 入参 JSON 的**字节数**（仅 size，**无内容**） |
| `result_bytes` | `usize` | 结果 JSON 的**字节数**（仅 size，**无内容**；`Err` 路径为 0） |

**仅元数据不变量**：三个 `Option` 字段用 `#[serde(skip_serializing_if = "Option::is_none")]`，故
`None` 时不出现在输出里。crate 自带单测把序列化后的 key 集合**锁死**为恰好这 9 个元数据键（断言里
显式排除 `arguments`/`args`/`result`/`content`/`text` 等载荷键），任何新增字段都必须在该测试里被
有意识地承认。

**`upstream` 归因安全修复**：`upstream` 现由下游对 `target_tool` 做**工具目录解析**取其真实 `server`
（`metatools::get_tool_details(snapshot, t).map(|def| def.server)`）得到，**不再** `split_once("__")` 切
client 提供的 `call_tool` 名。否则一个未知/构造的 `call_tool` 名（`ToolNotFound`）会注入一个无界、
attacker 可控的 `upstream` 前缀——既污染指标、又能灌爆 dashboard 的 `per_upstream` 维度。修复后未解析到的
调用 `upstream = None`（详见 [downstream-lib](./downstream-lib.md) / [downstream L3](../L3-details/downstream.md)）。

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

## 调用内容契约（`CallContent` + `CallContentSink`）

与仅元数据 `CallRecord` **物理隔离**的一条调用**内容**扇出通道：把一次调用的 args/result 文本捕获**仅入
dashboard 内存环**，使参数/结果内容**绝不**抵达 tracing/审计 sink。下游在元数据 `sinks` 扇出之后、且
`content_sinks` **非空**时才构造并扇出 `CallContent`（见 [downstream-lib](./downstream-lib.md)）。

### `struct CallContent`
```rust
#[derive(Debug, Clone)]
pub struct CallContent {
    pub args: String,
    pub args_truncated: bool,
    pub result: String,
    pub result_truncated: bool,
}
```
一次调用的内容载荷（args + result），**只**被捕获进 dashboard 内存环——与仅元数据 `CallRecord` 物理分离。
字段均为**已序列化、已截断**的文本（便于存储 / 子串搜索 / 在 `<pre>` 渲染）：`args` 是 JSON 文本；`result`
是序列化后的结果**或**上游错误纯文本（`Err` 路径）。`*_truncated` 标记是否触达上限。**注意**：与 `CallRecord`
不同，本类型**不** `Serialize`（dashboard 自有 `CallItem` 决定怎样以及是否对外暴露）。

| 字段 | 类型 | 含义 |
|------|------|------|
| `args` | `String` | 入参的 JSON 文本（已截断） |
| `args_truncated` | `bool` | `args` 是否被截断 |
| `result` | `String` | 成功结果的序列化文本，或失败时的上游错误纯文本（已截断） |
| `result_truncated` | `bool` | `result` 是否被截断 |

### `trait CallContentSink`
```rust
pub trait CallContentSink: Send + Sync {
    fn record(&self, meta: &CallRecord, content: &CallContent);
}
```
调用**内容**的扇出目标。**同时拿到**元数据 `CallRecord` 与 `CallContent`，故 dashboard 环可存一条富记录而
**不必重复**元数据字段。与 `CallSink` / `DiscoverySink` 一样，实现**必须非阻塞、且绝不 panic**。由 dashboard 的
`CallRingSink`（内存 ring）实现并仅装进 `content_sinks` 切片——**绝不**装进元数据 `sinks`，使内容永不入
tracing/审计。`Send + Sync` 使其可放进跨线程共享的 `Arc<[Arc<dyn CallContentSink>]>`。

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

## 发现追踪契约（`discovery.rs`，经 `lib.rs` re-export）

子系统 A（dashboard）的 **opt-in、与仅元数据 `CallRecord` 物理隔离**的搜索发现追踪：把 `search_tools` 的
`query → 命中工具+分数` 记录下来。**刻意与 `CallRecord` 分开**，使 query 文本/工具名**绝不**漏进
privacy-clean 的调用 sink（tracing / 审计 JSONL）。

### `struct DiscoveryHit`
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryHit { pub name: String, pub score: f32 }
```
一条 discovery 命中：命名空间化工具名 `name` 与检索相关性 `score`。

### `struct DiscoveryRecord`
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    pub ts_unix_ms: u64,
    pub query: String,
    pub top_k: usize,
    pub results: Vec<DiscoveryHit>,
    pub latency_ms: u64,
}
```
一次 `search_tools` 调用的发现追踪：原始 `query`、`top_k`、它召回的工具（带分数）与 `latency_ms`。
**同时 `Serialize + Deserialize`**——故可被 dashboard 落成 discovery JSONL 一行、再回放（与仅 `Serialize` 的
`CallRecord` 不同）。单测锁死序列化键集为 `{ts_unix_ms, query, top_k, results, latency_ms}`。

### `trait DiscoverySink`
```rust
pub trait DiscoverySink: Send + Sync {
    fn record(&self, rec: &DiscoveryRecord);
}
```
发现追踪的扇出目标，与 `CallSink` **并列但独立**。由 dashboard 的 `DiscoveryRingSink`（内存 ring + 可选
JSONL writer）实现。下游 `search_tools` 分支在 discovery 切片**非空**时构造一条 `DiscoveryRecord` 扇出
（空切片即不捕获）；装配仅在 `[dashboard].trace_queries = true` 时注入该 sink，故默认无追踪。`Send + Sync`
使其可放进跨线程共享的 `Arc<[Arc<dyn DiscoverySink>]>`。

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
- `crates/observe/src/lib.rs` `content_tests`：`CallContentSink::record` **同时收到** `CallRecord` 与
  `CallContent`（验证内容扇出契约把元数据与内容一并交付）。
- `crates/observe/tests/capture.rs`（需 `testkit`）：`CaptureSink` 按顺序记录多条 `CallRecord`。

> 谁产生记录、如何分类 `error_kind`、延迟测量基准见 L3：[downstream](../L3-details/downstream.md)；
> 组件视角见 L2：[observe](../L2-components/observe.md)。
