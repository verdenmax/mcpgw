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
    pub skipped: Vec<(String, String)>,   // 跳过的上游 + 原因（超时 / 调用错误 / 任务 panic，排序）
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
    backends: Backends,                        // 私有，retrieval 后端（embedder/chat/subagent_candidates），跨 rebuild 持有（保留缓存）
    rebuild_lock: Arc<Mutex<()>>,              // 私有，串行化重建
}
```
可廉价 `Clone` 的共享网关状态：`ArcSwap` 快照（读无锁）+ 上游注册表 + 策略名 + 检索后端 `Backends` + 重建锁。`Clone` 仅克隆内部 `Arc`，
所有克隆共享同一份状态。

### `GatewayState::new`
```rust
pub fn new(strategy_name: &str) -> Result<Self, GatewayError>
```
建空状态：用 `build_strategy(strategy_name, &Backends::default())` 新建策略、对空 `Catalog` `index`，装入
`ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))`；`backends` 为空（`Backends::default()`），注册表与重建锁均为空/新建。策略名未知（或需要
embedder/chat 却未提供）时返回 `Err(GatewayError::Strategy)`。

### `GatewayState::with_embedder`
```rust
pub fn with_embedder(strategy_name: &str, embedder: Arc<dyn Embedder>) -> Result<Self, GatewayError>
```
便捷封装：把 `embedder` 包进 `Backends { embedder: Some(embedder), ..Default::default() }` 后委托 `with_backends`，供 "vector"/"hybrid"
策略在每次 `rebuild_snapshot` 时复用（若是 `CachingEmbedder` 则缓存跨 rebuild 保留，仅嵌入新增工具）。策略构建失败返回
`Err(GatewayError::Strategy)`。

### `GatewayState::with_backends`
```rust
pub fn with_backends(strategy_name: &str, backends: Backends) -> Result<Self, GatewayError>
```
同 `new`，但持有任意 `retrieval::Backends`（`embedder` 供 "vector"/"hybrid"；`chat` + `subagent_candidates` 供 "subagent"），
经 `build_strategy(strategy_name, &backends)` 构建策略并跨 `rebuild_snapshot` 复用。`mcpgw` 启动期据 `strategy` 用 `build_backends` 装配后调用此入口。

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
   `tokio::time::timeout(handle.call_timeout(), handle.ingest_into(&mut local)).await`；同时把 `spawn` 返回的
   `task::Id` → 上游名记入 `names_by_id`（用于 panic 归因）。
3. `join_next` 收集：每个结果先经私有 `resolve_joined` 处理——任务 panic/取消（`JoinError`）按 `task::Id` 归因后
   降级为 `skipped("task failed: …")` + 一条 `warn`（归因缺失则记 `"<ingest task>"`），返回 `None` 即跳过、**绝不** re-panic；
   其余按 outcome：超时 → `skipped("ingest timed out")`；调用错误 → `skipped(err)`；成功 → 把 `local` 工具
   `upsert` 进最终 catalog 并记 `ingested`。两表均排序。
4. `build_strategy(&self.strategy_name, &self.backends)?`（未知名/缺 embedder 或 chat 则 `Err(GatewayError::Strategy)`）→
   `strat.index(&catalog)`。复用 state 持有的 `backends`，故 `CachingEmbedder` 的缓存跨 rebuild 保留。
5. `self.snapshot.store(Arc::new(GatewaySnapshot::new(catalog, strat)))` **原子换入**（build-then-swap），返回
   `RebuildSummary`。

读路径（`snapshot()`）全程无锁；重建经 `rebuild_lock` 串行化以保证 last-store-wins、不留陈旧快照；per-ingest
超时使 hung/慢上游被隔离进 `skipped`，不拖死重建；单个 ingest 任务 panic/取消亦经 `resolve_joined` 降级为
`skipped`（按 `task::Id` 归因），保住启动期（`prepare_state` 初次构建）与重建 worker 的崩溃隔离。

## `async fn run_rebuild_worker`
```rust
pub async fn run_rebuild_worker(state: GatewayState, mut rx: tokio::sync::mpsc::Receiver<String>)
```
排空 `rx`（`RebuildTrigger` 的接收端）：每收到一个触发就 `while rx.try_recv().is_ok() {}` 排空积压、**把一波突发
合并为一次 `rebuild_snapshot`**，并 `info!`/`warn!` 记录 `RebuildSummary`。channel 关闭（所有发送端 drop）时退出。
`serve` spawn 它处理上游 `tools/list_changed`。

> 详见 L3：[gateway](../L3-details/gateway.md)
