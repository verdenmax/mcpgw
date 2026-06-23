# L2 — `gateway` 组件

## 职责

网关的**状态与重建层**（M1-B.1，M1-B.2 扩展为并发摄取 + list_changed worker）：持有活的、可原子热替换的
`GatewaySnapshot`（经 `ArcSwap`），外加上游连接注册表 `UpstreamRegistry`，并负责**从上游重建快照**——把每个上游的
工具**并发**摄取进新 catalog、建索引、原子换入。读快照**无锁**；重建经 `Mutex` 串行化。它**不**实现元工具函数本身
（复用 `metatools`），也**不**起 MCP server 或做 eager-connect（那是 M1-B.2 的 `mcpgw serve` + `upstream::connect`）。
**（子系统 B）** `GatewayState` 另持一个可选 JSON 持久化的 `DisableSet`（运行时禁用集，默认空 → 行为不变），
`rebuild_snapshot` 读它**跳过被禁用上游的 ingest 与被禁用的单工具**，使被禁用项不进新快照（这是隐藏式禁用的唯一过滤点）。
**（子系统 C）** `GatewayState` 另提供 `reconcile_upstreams(old, new, trigger)`：按 `name` 对上游配置做**纯三向 diff**
后协调注册表（删除/重连/不动）并末尾 `rebuild_snapshot`，供 dashboard 的在线改配做**上游热重载**（best-effort，
复用既有 `connect_all`/`registry.remove`，与禁用过滤天然组合）。

## 公开接口

### 错误 `GatewayError`（`lib.rs`）
`#[derive(thiserror::Error)]` 枚举（取代早期的裸 `String` 错误）：

- `Strategy(String)` — 检索策略未实现/构建失败（来自 `retrieval::build_strategy`）。

### 类型 `RebuildSummary`（`lib.rs`）
一次重建的遥测，`#[derive(Debug, Default, Clone, PartialEq)]`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `ingested` | `Vec<String>` | 工具被摄取进新快照的上游名（排序） |
| `skipped` | `Vec<(String, String)>` | 本次跳过的上游 + 简短原因（超时 / 调用错误）（排序） |

### 类型 `GatewayState`（`lib.rs`）
可廉价 `Clone` 的共享网关状态：`ArcSwap` 快照（读无锁）+ 上游注册表 + 策略名 + 重建锁 + 最近重建摘要（`ArcSwapOption`，读无锁）+ 运行时禁用集 `Arc<DisableSet>`（默认空）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(strategy_name: &str) -> Result<Self, GatewayError>` | 建空状态（无上游、空 catalog），用给定策略名（如 `"bm25"`）；策略未实现则返回 `Err(GatewayError::Strategy)` |
| `registry` | `(&self) -> &UpstreamRegistry` | 上游注册表（`serve` 的 eager-connect 填充；测试注入 mock handle） |
| `snapshot` | `(&self) -> Arc<GatewaySnapshot>` | 加载当前快照（**无锁**，`load_full`） |
| `last_summary` | `(&self) -> Option<Arc<RebuildSummary>>` | 最近一次成功重建的 `RebuildSummary`（**无锁**），首次重建前为 `None`；供 dashboard 只读读取已摄取/被跳过的上游 |
| `with_disabled` | `(self, disabled: Arc<DisableSet>) -> Self` | **（子系统 B）** 装配期注入运行时禁用集（须在**首次 rebuild 之前**），替换默认空集；返回 `Self` 便于链式构造。cheap-clone 的 `Arc` 与 dashboard 经同一 `GatewayState` 共享 |
| `disabled` | `(&self) -> &DisableSet` | **（子系统 B）** 借用运行时禁用集（rebuild 读、dashboard admin API 改、`GET /api/disabled` 读快照） |
| `disabled_arc` | `(&self) -> Arc<DisableSet>` | **（子系统 B）** `Arc` 克隆，供需跨 `.await` move 的调用方（如 dashboard admin handler 在 `spawn_blocking` 里跑同步持久化变更） |
| `rebuild_snapshot` | `async (&self) -> Result<RebuildSummary, GatewayError>` | 从注册表**并发**摄取（每个 ingest 受该 handle 的 `call_timeout` 约束）→ 建索引 → 原子换入新快照；经重建锁串行化；返回 `RebuildSummary` 并存为 `last_summary`。**读 `disabled`：ingest 前跳过被禁用上游（不在 `summary.ingested`、连 `tools/list` 都不发）、upsert 时跳过被禁用单工具** |
| `reconcile_upstreams` | `async (&self, old: &[UpstreamConfig], new: &[UpstreamConfig], trigger: RebuildTrigger) -> ReconcileSummary` | **（子系统 C）** 按 `name` 三向 diff `old`/`new`：removed → `registry.remove`、added·changed → `upstream::connect::connect_all`（复用 eager-connect 路径）、unchanged → 不动连接；末尾单次 `rebuild_snapshot`。**best-effort**——连接失败记 `connect_failures`、不中止其它/不回滚；纯 no-op（无增删）提前返回不 rebuild。供 dashboard 在线改配做上游热重载 |

### 类型 `DisableSet` / `DisabledSnapshot`（`disable.rs`，子系统 B）
运行时**临时禁用集**：内存 `RwLock<{upstreams, tools}>`（两个 `BTreeSet<String>` → 天然有序）+ 可选 JSON 持久化路径。
被禁用的上游 namespace / qualified 工具名经 `rebuild_snapshot` 过滤后从快照消失，对下游表现为 `ToolNotFound`（隐藏式
语义，`metatools`/`downstream` 零改动）。`Default` = 空集、无持久化（默认 `GatewayState` 所持）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `load_or_new` | `(path: Option<PathBuf>) -> Self` | 有 path 且文件存在则读入；缺文件 → 空集（正常非错误）；坏 JSON / 坏 UTF-8 → 空集 + `warn!`（自愈，绝不 panic/挡启动）。陈旧名保留（过滤永不命中、`enable` 可清） |
| `is_upstream_disabled` / `is_tool_disabled` | `(&self, name/qualified: &str) -> bool` | 过滤判定（`rebuild_snapshot` 调用，读锁） |
| `disable_upstream` / `enable_upstream` / `disable_tool` / `enable_tool` | `(&self, name: &str) -> bool` | 变更，返回 `changed`；**仅 changed 时**持久化（best-effort 原子写） |
| `snapshot` | `(&self) -> DisabledSnapshot` | 有序快照——`GET /api/disabled` 响应体 + 持久化形态 |

`DisabledSnapshot { upstreams: Vec<String>, tools: Vec<String> }`（`Serialize`/`Deserialize`，有序）：开放只读端点
`GET /api/disabled` 的 body 与磁盘 JSON 形态（空集即 `{ "upstreams": [], "tools": [] }`）。

### 类型 `ReconcileSummary`（`lib.rs`，子系统 C）
`reconcile_upstreams` 的结果，`#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]`，序列化进 dashboard 的 `ApplyResult`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `added` | `Vec<String>` | 本次新增（`new` 有、`old` 无）尝试连接的上游名 |
| `removed` | `Vec<String>` | 本次移除（`old` 有、`new` 无）从 registry 删除的上游名 |
| `reconnected` | `Vec<String>` | 同名但 `UpstreamConfig` 改变、尝试重连的上游名 |
| `connect_failures` | `Vec<(String, String)>` | (名, 错误)：`added`/`reconnected` 里**实际连接失败**者 |

> `added`/`reconnected` 是**计划意图**：同时出现在 `connect_failures` 里的项**未真正（重）连**（changed 上游连接失败时旧连接保留）。
> 取**真正生效集**须把 `added`/`reconnected` 与 `connect_failures` 交叉看。私有 `plan_upstream_reconcile(old, new) -> ReconcilePlan` 是被重点测试的纯三向 diff。

### 函数 `run_rebuild_worker`（`lib.rs`）

| 函数 | 签名 | 说明 |
|------|------|------|
| `run_rebuild_worker` | `async (state: GatewayState, rx: mpsc::Receiver<String>)` | 排空 `rx`、**每波突发合并为一次重建**；channel 关闭（所有 `RebuildTrigger` 发送端 drop）时退出。`serve` spawn 它来处理上游 `tools/list_changed` |

## 依赖

- 内部：`metatools`（`GatewaySnapshot`）、`catalog`（`Catalog`）、`retrieval`（`build_strategy`）、`upstream`
  （`UpstreamRegistry` / `UpstreamHandle`；**子系统 C** 另用 `connect::connect_all`）、`config`（**子系统 C**：
  `reconcile_upstreams` 的 `UpstreamConfig` 三向 diff）。
- 外部：`arc-swap`（`ArcSwap` 热替换）、`tokio`（`sync::Mutex` 重建锁、`task::JoinSet` 并发摄取、`mpsc` 触发
  channel）、`thiserror`（`GatewayError`）、`tracing`（重建日志）、`serde`/`serde_json`（`DisabledSnapshot` 序列化
  + `DisableSet` 的 JSON 持久化）；`DisableSet` 内部用 `std::sync::RwLock` + `BTreeSet`（有序、无第三方锁）。

## 关键不变量

- **build-then-swap**：先在临时变量里把新 catalog/策略完全建好，再 `store` 原子换入；切换前的快照对读者始终完整。
- **读无锁**：`snapshot()` 只 `ArcSwap::load_full`，从不触碰重建锁；持有的旧 `Arc<GatewaySnapshot>` 在被换出后
  仍可安全读到生命周期结束。
- **重建经 `Mutex` 串行化**：`rebuild_snapshot` 全程持 `rebuild_lock`，使并发触发不会把陈旧快照留作最终态
  （last-store-wins）。
- **单上游失败/挂起隔离**：每个上游在独立任务里摄取、各自受 `call_timeout` 约束；超时/报错的上游被记入
  `skipped`，**绝不**阻塞其余上游或拖死整次重建（这彻底修复了 B.1 串行摄取里 hung 上游饿死后续重建的隐患）。
- **worker 合并**：`run_rebuild_worker` 把一波连续触发 coalesce 成单次重建，避免突发抖动放大成多次无谓重建。
- **禁用过滤只在 rebuild**（子系统 B）：被禁用上游/工具仅在 `rebuild_snapshot` 时被剔出新快照——`metatools`/`downstream`
  读路径零改动；已在途的调用可能在禁用生效后**再完成一次**（check-then-call 竞态，无状态泄漏），下次重建后彻底消失。
  `DisableSet` 默认空集时整条路径行为与禁用功能引入前完全一致。
- **上游热重载 best-effort 且与禁用组合**（子系统 C）：`reconcile_upstreams` 的某上游连接失败只记 `connect_failures`、
  **不**中止其它上游、**不**回滚已落盘配置；其末尾 `rebuild_snapshot` 仍读 `DisableSet` 过滤，故热加的禁用上游照样隐藏。

## 向下导航

- 内部细节见 L3：[gateway](../L3-details/gateway.md)
- 逐文件 API 见 L4：[lib](../L4-api/gateway-lib.md)
- 快照/元工具见：[metatools L2](./metatools.md)
