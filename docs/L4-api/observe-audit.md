# L4 — `crates/observe/src/audit.rs` API

源文件：`crates/observe/src/audit.rs`。**可选的 JSONL 审计落盘 sink**（M6.T3）：`JsonlSink` 实现
`CallSink`，把每条**仅元数据**的 `CallRecord` 序列化成一行 JSON 并 `try_send` 进**有界 channel**；一个
专用 OS 线程（writer）持有接收端做阻塞文件 I/O（append + 缓冲写 + flush + fsync），故调用热路径**绝不
阻塞**。整块仍是 **std-only（无 tokio）**——`observe` 不因此新增任何依赖。它与 `TracingSink` 一样只是
插进同一份 `Arc<[Arc<dyn CallSink>]>` 的又一个 sink（装配见 [mcpgw-main](./mcpgw-main.md)）。

## `const AUDIT_CHANNEL_CAPACITY`
```rust
pub const AUDIT_CHANNEL_CAPACITY: usize = 1024;
```
`JsonlSink::record` 与 writer 线程之间**有界** channel 的容量。channel 满时记录被**丢弃**（计数 + 限频
warn，见下），调用路径永不被阻塞。`mcpgw serve` 用它做 `spawn_writer` 的 `capacity` 实参。

## `struct JsonlSink`
```rust
#[derive(Clone)]
pub struct JsonlSink {
    tx: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}
```
一个 `CallSink`：经背景 writer 线程把每条记录作为一行 JSON 追加落盘。`#[derive(Clone)]`——克隆**共享同一个
`SyncSender` 与同一个 `Arc<AtomicU64>` 丢弃计数**，故任意 clone 走同一条 channel、同一份计数（HTTP 传输按
会话铸造多个 `GatewayServer`，各自持一份 clone）。

### `JsonlSink::record`（impl `CallSink`）
```rust
fn record(&self, rec: &CallRecord)
```
1. `serde_json::to_string(rec)` 序列化**单条 `CallRecord`**为一行 JSON。序列化失败（极罕见）→ `tracing::warn!`
   后**直接返回**（丢弃该条，绝不 panic）。
2. `self.tx.try_send(line)`（**非阻塞**）：
   - `Ok(())` —— 入队成功，由 writer 线程异步落盘。
   - `Err(Full)` / `Err(Disconnected)` —— channel 满或 writer 已退出 → `dropped.fetch_add(1)`，并在累计丢弃数
     `n` **首次（`n == 1`）及之后每个 2 的幂次**（`n.is_power_of_two()`）打一条 `tracing::warn!(dropped = n, …)`。
     **限频**保证突发丢弃不会刷屏日志。

契约同 `CallSink`：**非阻塞、绝不 panic**；只发**元数据**（`CallRecord` 按类型构造即不可能带 payload）。

### `JsonlSink::dropped_count`
```rust
pub fn dropped_count(&self) -> u64
```
至今因 channel 满/断连而被丢弃的记录数（`Relaxed` 读取共享的 `Arc<AtomicU64>`）。供测试/诊断断言。

## `struct AuditWriter`
```rust
pub struct AuditWriter {
    handle: JoinHandle<()>,
}
```
背景 writer 线程的句柄。**只持 `JoinHandle`，不持任何 sender**——这是关键设计：channel 的断连（进而触发
writer drain）**只能由所有 `JsonlSink` clone 全部 drop 来达成**，`AuditWriter` 自己不会让 channel 一直存活。

### `AuditWriter::join`
```rust
pub fn join(self)
```
**按值**消费句柄并阻塞，直到 writer 线程结束——即：所有 `JsonlSink` clone 已 drop（channel 断连）→ writer
把队列 FIFO drain 完 → 最终 `flush` → `sync_all`（fsync）→ 退出。`join` 内部忽略线程 panic（`let _ =`）。
`mcpgw serve` 在关停时经 `spawn_blocking` + `tokio::time::timeout(AUDIT_DRAIN_TIMEOUT, …)` 调它（见
[mcpgw-main](./mcpgw-main.md)）。

## `fn spawn_writer`
```rust
pub fn spawn_writer(path: &Path, capacity: usize) -> std::io::Result<(JsonlSink, AuditWriter)>
```
打开 `path`（`create(true).append(true)`——不存在则建、存在则**追加**），建容量 `capacity` 的有界 channel，
并**命名为 `audit-writer` 的 OS 线程**里跑 writer loop；返回配对的 `(JsonlSink, AuditWriter)`。

- **`Err`** 当文件无法打开（如父目录不存在/权限不足）**或**线程无法 spawn——调用方据此 **fail-fast**
  （`mcpgw serve` 在 `[audit].enabled` 时 `map_err(...)?`，开不了审计文件就**拒绝启动**）。
- 内部经 `pub(crate) fn channel(capacity)` 构造 sink + 接收端；该 `channel` 仅对**测试**暴露，使其能持有未读
  接收端、确定性地撑满 channel 来验证「满则丢弃」路径。

## writer loop 语义（私有 `run_writer`）

```rust
fn run_writer(rx: Receiver<String>, file: File)   // BufWriter<File> 包裹
```
- **批量 drain + 每批 flush**：阻塞 `rx.recv()` 取首条 → 写一行 → `while rx.try_recv()` 把当下排队的全部续写
  （摊销系统调用）→ 每批末尾 `w.flush()`（把 `BufWriter` 推到 OS）。
- **fsync 只在干净 drain 时一次**：`rx.recv()` 返回 `Err`（**所有 sender 已 drop** → channel 断连）即退出循环，
  做**最终 `flush` + `file.sync_all()`（fsync，落到稳定存储）**后线程结束。正常路径下 fsync 只发生一次（退出前），
  不在每批里。
- **durability 取舍**：运行期每批只 `flush`（推到 OS page cache），稳定落盘的 fsync 只在干净 drain 时做一次。
  即：进程**被强杀（SIGKILL）或断电**而未走干净 drain 时，已 `flush` 但未 fsync 的批次可能丢失（强杀下根本不会
  fsync）。需要更强 durability 的场景应另加周期性 fsync——本任务范围外。
- **写失败不退出 → 自愈**：每行写、每次 flush 的 `io::Error` 都只走 `rate_limited_write_error`（累计计数，**首次
  及每个 2 的幂次** warn 一条），**writer 继续运行**——瞬时故障（如一度写满后又清出空间的磁盘）能**自愈**，不会
  因一次写错而永久停摆、丢掉后续所有审计。

## 仅元数据不变量

`record()` 只 `serde_json::to_string` 一个 **`CallRecord`**（其形状见 [observe-lib](./observe-lib.md)：9 个元数据键、
只含 `arg_bytes`/`result_bytes` 等 **size**，**无任何参数/结果载荷**）。因此审计行**永不**含 secret/PII。crate
单测显式断言审计行**不含** `arguments` 等 payload 键。

## 测试
- `crates/observe/src/audit.rs` 单测：
  - `writes_n_records_as_valid_jsonl_and_drains_on_drop` —— `spawn_writer` 写 5 条、`drop(sink)` 触发
    drain/flush/fsync/退出后 `join`，断言文件恰 5 行合法 JSON、键为 `call_tool` 元数据且**无 `arguments`**。
  - `channel_full_increments_dropped_without_blocking` —— 持容量 1 的未读接收端，撑满后再发两条 → `dropped_count() == 2`、不阻塞。
  - `spawn_writer_open_failure_returns_err` —— 不存在的目录路径 → `Err`（fail-fast 入口）。
- `crates/mcpgw/tests/audit.rs`（集成）：`serve_with_audit_enabled_writes_jsonl_for_a_meta_tool_call` —— 起真实
  `mcpgw serve --config`（`[audit].enabled`），经 stdio 客户端调一次 `search_tools`，断开触发优雅 drain 后，断言审计
  文件首行是 `meta_tool == "search_tools"` / `outcome == "ok"` 的元数据行且**不含 payload**。

> 装配与关停时的有界 drain 见 L4：[mcpgw-main](./mcpgw-main.md)；记录形状/`CallSink` 契约见
> [observe-lib](./observe-lib.md)；组件视角见 L2：[observe](../L2-components/observe.md)。
