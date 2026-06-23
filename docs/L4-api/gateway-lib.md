# L4 — `crates/gateway/src/lib.rs` API

源文件：`crates/gateway/src/lib.rs`。活的、可原子热替换的 `GatewaySnapshot` 状态 + 上游注册表 + 重建逻辑 +
list_changed 重建 worker + 上游热重载对账（`reconcile_upstreams`，子系统 C）。

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

## `struct ReconcileSummary`
```rust
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReconcileSummary {
    pub added: Vec<String>,                       // 新增并尝试连接的上游名
    pub removed: Vec<String>,                      // 已从 registry 摘除的上游名
    pub reconnected: Vec<String>,                 // 配置变更、已尝试重连的上游名
    pub connect_failures: Vec<(String, String)>,  // (上游名, 连接错误)——连接失败者
}
```
一次**上游热重载**（`reconcile_upstreams`，子系统 C）的遥测，`Serialize` 后嵌进 dashboard 的 `ApplyResult.upstreams`。
**`added`/`reconnected` 是"意图"**：表示该上游被纳入本次 (re)connect 批，**不**保证连接成功（*changed* 上游连失败时旧连接仍保留）；
**真正生效的集合须用 `added`/`reconnected` 减去 `connect_failures`**。`removed` 则确定已摘除。

## `struct DisableSet` / `struct DisabledSnapshot`（`disable.rs`，`pub use` 自 `lib.rs`）
```rust
pub struct DisableSet { /* RwLock<{upstreams, tools}: BTreeSet<String>> + Option<PathBuf> */ }

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DisabledSnapshot { pub upstreams: Vec<String>, pub tools: Vec<String> }
```
运行时**临时禁用集**（子系统 B），经 `pub use disable::{DisableSet, DisabledSnapshot}` 暴露到 crate 根。内存
`BTreeSet`（有序）+ 可选 JSON 持久化。被禁用的上游 namespace / qualified 工具名经 `rebuild_snapshot` 过滤后从快照消失
（隐藏式语义）。`Default` = 空集、无持久化。主要 API：

- `load_or_new(path: Option<PathBuf>) -> Self`：有 path 且文件在则读入；缺文件 → 空集；坏 JSON/UTF-8 → 空集 + `warn!`（自愈，绝不 panic）。
- `is_upstream_disabled(&str) -> bool` / `is_tool_disabled(&str) -> bool`：过滤判定（`rebuild_snapshot` 用）。
- `disable_upstream`/`enable_upstream`/`disable_tool`/`enable_tool(&str) -> bool`：变更，返回 `changed`；仅 changed 时 best-effort 原子写盘（temp→fsync→rename）。
- `snapshot(&self) -> DisabledSnapshot`：有序快照——`GET /api/disabled` 响应体 + 磁盘形态。

## `struct GatewayState`
```rust
#[derive(Clone)]
pub struct GatewayState {
    snapshot: Arc<ArcSwap<GatewaySnapshot>>,   // 私有
    registry: UpstreamRegistry,                // 私有
    strategy_name: Arc<str>,                   // 私有
    backends: Backends,                        // 私有，retrieval 后端（embedder/chat/subagent_candidates），跨 rebuild 持有（保留缓存）
    rebuild_lock: Arc<Mutex<()>>,              // 私有，串行化重建
    last_summary: Arc<ArcSwapOption<RebuildSummary>>, // 私有，最近一次重建摘要（供 dashboard 只读读取）
    disabled: Arc<DisableSet>,                 // 私有，运行时禁用集（默认空），每次 rebuild 读以跳过禁用上游/工具
}
```
可廉价 `Clone` 的共享网关状态：`ArcSwap` 快照（读无锁）+ 上游注册表 + 策略名 + 检索后端 `Backends` + 重建锁 +
最近重建摘要（`ArcSwapOption`，读无锁）+ 运行时禁用集 `Arc<DisableSet>`（默认空 → 行为不变）。`Clone` 仅克隆内部
`Arc`，所有克隆共享同一份状态（含同一 `DisableSet`，故 dashboard admin 写入对 rebuild 立即可见）。

### `GatewayState::new`
```rust
pub fn new(strategy_name: &str) -> Result<Self, GatewayError>
```
建空状态：用 `build_strategy(strategy_name, &Backends::default())` 新建策略，**不在构造时索引**——直接把空 `Catalog`
装入 `ArcSwap::from_pointee(GatewaySnapshot::new(empty, strat))`（`index` 仅由首个 `rebuild_snapshot` 调用，故首次重建前
`search` 返回空）；`backends` 为空（`Backends::default()`），注册表与重建锁均为空/新建。策略名未知（或需要
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

### `GatewayState::last_summary`
```rust
pub fn last_summary(&self) -> Option<Arc<RebuildSummary>>
```
`self.last_summary.load_full()`：**无锁**返回**最近一次** `rebuild_snapshot` 成功提交的 `RebuildSummary`
（含 `ingested`/`skipped`），首次重建前为 `None`。每次重建在 swap 快照后 `store(Some(Arc::new(summary)))`。
供 dashboard 的 `/api/upstreams` 等只读读取已摄取/被跳过的上游归因。无错误。

### `GatewayState::with_disabled`
```rust
pub fn with_disabled(self, disabled: Arc<DisableSet>) -> Self
```
**（子系统 B）** 装配期注入运行时禁用集，替换默认空集，返回 `Self`（builder 风格，便于链式构造）。须在**首次
`rebuild_snapshot` 之前**调用（`mcpgw` 的 `prepare_state` 在 `connect_all` + 初次 rebuild 前调），使持久化的禁用项
从启动即生效。`Arc` 与 dashboard 经同一 `GatewayState`（cheap-clone）共享。

### `GatewayState::disabled`
```rust
pub fn disabled(&self) -> &DisableSet
```
**（子系统 B）** 借用运行时禁用集：`rebuild_snapshot` 读它过滤、dashboard admin handler 改它、`GET /api/disabled`
读它的 `snapshot()`。无锁（`DisableSet` 内部自带 `RwLock`）。

### `GatewayState::disabled_arc`
```rust
pub fn disabled_arc(&self) -> Arc<DisableSet>
```
**（子系统 B）** `Arc` 克隆，供需跨 `.await` `move` 的调用方——dashboard admin handler 在 `spawn_blocking` 里跑同步、
会 `fsync` 的持久化变更，需把 `Arc<DisableSet>` move 进闭包。

### `GatewayState::rebuild_snapshot`
```rust
pub async fn rebuild_snapshot(&self) -> Result<RebuildSummary, GatewayError>
```
从注册表**并发**重建快照（持 `rebuild_lock` 串行化）：

1. `rebuild_lock.lock().await` 取得守卫。
2. 对 `registry.server_names()` 的每个上游：**先 `self.disabled.is_upstream_disabled(name)` → 跳过被禁用上游**
   （连任务都不 `spawn`、不发 `tools/list`、不进 `ingested`/`skipped`）；否则 `spawn` 一个 `JoinSet` 任务，在**任务私有**
   `Catalog` 上 `tokio::time::timeout(handle.call_timeout(), handle.ingest_into(&mut local)).await`；同时把 `spawn` 返回的
   `task::Id` → 上游名记入 `names_by_id`（用于 panic 归因）。
3. `join_next` 收集：每个结果先经私有 `resolve_joined` 处理——任务 panic/取消（`JoinError`）按 `task::Id` 归因后
   降级为 `skipped("task failed: …")` + 一条 `warn`（归因缺失则记 `"<ingest task>"`），返回 `None` 即跳过、**绝不** re-panic；
   其余按 outcome：超时 → `skipped("ingest timed out")`；调用错误 → `skipped(err)`；成功 → 把 `local` 工具
   `upsert` 进最终 catalog（**`upsert` 前 `self.disabled.is_tool_disabled(tool.qualified_name())` → 跳过被禁用的单工具**）
   并记 `ingested`。两表均排序。
4. `build_strategy(&self.strategy_name, &self.backends)?`（未知名/缺 embedder 或 chat 则 `Err(GatewayError::Strategy)`）→
   `strat.index(&catalog)`。复用 state 持有的 `backends`，故 `CachingEmbedder` 的缓存跨 rebuild 保留。
5. `self.snapshot.store(Arc::new(GatewaySnapshot::new(catalog, strat)))` **原子换入**（build-then-swap），返回
   `RebuildSummary`。

读路径（`snapshot()`）全程无锁；重建经 `rebuild_lock` 串行化以保证 last-store-wins、不留陈旧快照；per-ingest
超时使 hung/慢上游被隔离进 `skipped`，不拖死重建；单个 ingest 任务 panic/取消亦经 `resolve_joined` 降级为
`skipped`（按 `task::Id` 归因），保住启动期（`prepare_state` 初次构建）与重建 worker 的崩溃隔离。

### `GatewayState::reconcile_upstreams`
```rust
pub async fn reconcile_upstreams(
    &self,
    old: &[config::UpstreamConfig],
    new: &[config::UpstreamConfig],
    trigger: upstream::connection::RebuildTrigger,
) -> ReconcileSummary
```
**（子系统 C：上游热重载）** 把活的上游注册表对账到一份新配置，供 dashboard 的 `PUT /api/admin/config` 在落盘后调用。流程：

1. **纯三向 diff**：私有 `plan_upstream_reconcile(old, new)` 按**名**比对 → `ReconcilePlan{ removed, to_connect, added, reconnected }`：
   仅在 `old` 的 → `removed`；仅在 `new` 的 → `added`（+入 `to_connect`）；两边都有但**配置不等**（`prev != u`）→ `reconnected`
   （+入 `to_connect`）；**完全相同 → 原连接不动**（不重连）。
2. **no-op 早退**：若 `removed` 与 `to_connect` 均空（纯无变更）→ 直接返回 `ReconcileSummary::default()`，**跳过 remove/connect/rebuild**。
3. **应用（best-effort）**：对 `removed` 逐个 `registry.remove(name)`；`to_connect` 非空时 `upstream::connect::connect_all(
   registry, &to_connect, trigger)`（复用 eager-connect 路径），其 `skipped` 即 `connect_failures`。
4. **单次 rebuild**：`self.rebuild_snapshot().await`（**忽略其 `Result`**——已用 `let _ =`，对账不因重建错误而 panic）；
   rebuild 会施加 M4 的禁用过滤，故热重载与运行时禁用**组合**正确。
5. 返回 `ReconcileSummary{ added, removed, reconnected, connect_failures }`。

**best-effort**：单个上游连接失败只记进 `connect_failures`、**绝不**中止其余上游或回滚（已落盘的配置不动）；`added`/`reconnected`
是意图、须与 `connect_failures` 交叉核对（见上 `ReconcileSummary`）。dashboard 据此把 `applied_upstreams` 基线只更新为
**连接成功**者，使对同一份配置再次 PUT 会重试仍失败的上游。详见 [gateway L3 上游热重载](../L3-details/gateway.md)。

## `async fn run_rebuild_worker`
```rust
pub async fn run_rebuild_worker(state: GatewayState, mut rx: tokio::sync::mpsc::Receiver<String>)
```
排空 `rx`（`RebuildTrigger` 的接收端）：每收到一个触发就 `while rx.try_recv().is_ok() {}` 排空积压、**把一波突发
合并为一次 `rebuild_snapshot`**，并 `info!`/`warn!` 记录 `RebuildSummary`。channel 关闭（所有发送端 drop）时退出。
`serve` spawn 它处理上游 `tools/list_changed`。

> 详见 L3：[gateway](../L3-details/gateway.md)
