# L4 — `crates/gateway/src/lib.rs` API

源文件：`crates/gateway/src/lib.rs`。活的、可原子热替换的 `GatewaySnapshot` 状态 + 上游注册表 + 重建逻辑 +
list_changed 重建 worker。

## `enum GatewayError`
```rust
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("unknown retrieval strategy: {0}")]
    Strategy(String),
}
```
`new` / `rebuild_snapshot` 的错误类型（取代早期裸 `String`）。当前仅 `Strategy`（检索策略未实现/构建失败）。

## `struct RebuildSummary`
```rust
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RebuildSummary {
    pub ingested: Vec<String>,            // 工具被摄取进新快照的上游名（排序）
    pub skipped: Vec<(String, String)>,   // 跳过的上游 + 原因（超时 / 调用错误，排序）
}
```
一次重建的遥测。

## `struct GatewayState`
```rust
#[derive(Clone)]
pub struct GatewayState {
    snapshot: Arc<ArcSwap<GatewaySnapshot>>,   // 私有
    registry: UpstreamRegistry,                // 私有
    strategy_name: Arc<str>,                   // 私有
    rebuild_lock: Arc<Mutex<()>>,              // 私有，串行化重建
}
```
可廉价 `Clone` 的共享网关状态：`ArcSwap` 快照（读无锁）+ 上游注册表 + 策略名 + 重建锁。`Clone` 仅克隆内部 `Arc`，
所有克隆共享同一份状态。

### `GatewayState::new`
```rust
pub fn new(strategy_name: &str) -> Result<Self, GatewayError>
```
建空状态：用 `build_strategy(strategy_name)` 新建策略、对空 `Catalog` `index`，装入
`ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))`；注册表与重建锁均为空/新建。策略名未实现时返回
`Err(GatewayError::Strategy)`。

### `GatewayState::registry`
```rust
pub fn registry(&self) -> &UpstreamRegistry
```
返回上游注册表引用（`serve` 的 eager-connect 填充；测试注入 mock handle）。无错误。

### `GatewayState::snapshot`
```rust
pub fn snapshot(&self) -> Arc<GatewaySnapshot>
```
`self.snapshot.load_full()`：**无锁**加载当前快照的 `Arc` 克隆。无错误。

### `GatewayState::rebuild_snapshot`
```rust
pub async fn rebuild_snapshot(&self) -> Result<RebuildSummary, GatewayError>
```
从注册表**并发**重建快照（持 `rebuild_lock` 串行化）：

1. `rebuild_lock.lock().await` 取得守卫。
2. 对 `registry.server_names()` 的每个上游 `spawn` 一个 `JoinSet` 任务，在**任务私有** `Catalog` 上
   `tokio::time::timeout(handle.call_timeout(), handle.ingest_into(&mut local)).await`。
3. `join_next` 收集：超时 → `skipped("ingest timed out")`；调用错误 → `skipped(err)`；成功 → 把 `local` 工具
   `upsert` 进最终 catalog 并记 `ingested`。两表均排序。
4. `build_strategy(&self.strategy_name)?`（未实现则 `Err(GatewayError::Strategy)`）→ `strat.index(&catalog)`。
5. `self.snapshot.store(Arc::new(GatewaySnapshot::new(catalog, strat)))` **原子换入**（build-then-swap），返回
   `RebuildSummary`。

读路径（`snapshot()`）全程无锁；重建经 `rebuild_lock` 串行化以保证 last-store-wins、不留陈旧快照；per-ingest
超时使 hung/慢上游被隔离进 `skipped`，不拖死重建。

## `async fn run_rebuild_worker`
```rust
pub async fn run_rebuild_worker(state: GatewayState, mut rx: tokio::sync::mpsc::Receiver<String>)
```
排空 `rx`（`RebuildTrigger` 的接收端）：每收到一个触发就 `while rx.try_recv().is_ok() {}` 排空积压、**把一波突发
合并为一次 `rebuild_snapshot`**，并 `info!`/`warn!` 记录 `RebuildSummary`。channel 关闭（所有发送端 drop）时退出。
`serve` spawn 它处理上游 `tools/list_changed`。

> 详见 L3：[gateway](../L3-details/gateway.md)
