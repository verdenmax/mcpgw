# L4 — `crates/gateway/src/lib.rs` API

源文件：`crates/gateway/src/lib.rs`。活的、可原子热替换的 `GatewaySnapshot` 状态 + 上游注册表 + 重建逻辑。

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
pub fn new(strategy_name: &str) -> Result<Self, String>
```
建空状态：用 `build_strategy(strategy_name)` 新建策略、对空 `Catalog` `index`，装入
`ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))`；注册表与重建锁均为空/新建。策略名未实现时返回
`Err(String)`（来自 `StrategyError::to_string()`）。

### `GatewayState::registry`
```rust
pub fn registry(&self) -> &UpstreamRegistry
```
返回上游注册表引用（B.2 的 eager-connect 填充；测试注入 mock handle）。无错误。

### `GatewayState::snapshot`
```rust
pub fn snapshot(&self) -> Arc<GatewaySnapshot>
```
`self.snapshot.load_full()`：**无锁**加载当前快照的 `Arc` 克隆。无错误。

### `GatewayState::rebuild_snapshot`
```rust
pub async fn rebuild_snapshot(&self) -> Result<(), String>
```
从注册表重建快照（持 `rebuild_lock` 串行化）：

1. `rebuild_lock.lock().await` 取得守卫。
2. 新建空 `Catalog`，对 `registry.server_names()` 的每个上游 `handle.ingest_into(&mut catalog).await`；单个失败仅
   `tracing::warn!` + skip（**错误隔离**）。
3. `build_strategy(&self.strategy_name)?`（未实现则 `Err(String)`）→ `strat.index(&catalog)`。
4. `self.snapshot.store(Arc::new(GatewaySnapshot::new(catalog, strat)))` **原子换入**（build-then-swap）。

读路径（`snapshot()`）全程无锁；重建经 `rebuild_lock` 串行化以保证 last-store-wins、不留陈旧快照。

> 详见 L3：[gateway](../L3-details/gateway.md)
