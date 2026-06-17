# L2 — `gateway` 组件

## 职责

网关的**状态与重建层**（M1-B.1，M1-B.2 扩展为并发摄取 + list_changed worker）：持有活的、可原子热替换的
`GatewaySnapshot`（经 `ArcSwap`），外加上游连接注册表 `UpstreamRegistry`，并负责**从上游重建快照**——把每个上游的
工具**并发**摄取进新 catalog、建索引、原子换入。读快照**无锁**；重建经 `Mutex` 串行化。它**不**实现元工具函数本身
（复用 `metatools`），也**不**起 MCP server 或做 eager-connect（那是 M1-B.2 的 `mcpgw serve` + `upstream::connect`）。

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
可廉价 `Clone` 的共享网关状态：`ArcSwap` 快照（读无锁）+ 上游注册表 + 策略名 + 重建锁 + 最近重建摘要（`ArcSwapOption`，读无锁）。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(strategy_name: &str) -> Result<Self, GatewayError>` | 建空状态（无上游、空 catalog），用给定策略名（如 `"bm25"`）；策略未实现则返回 `Err(GatewayError::Strategy)` |
| `registry` | `(&self) -> &UpstreamRegistry` | 上游注册表（`serve` 的 eager-connect 填充；测试注入 mock handle） |
| `snapshot` | `(&self) -> Arc<GatewaySnapshot>` | 加载当前快照（**无锁**，`load_full`） |
| `last_summary` | `(&self) -> Option<Arc<RebuildSummary>>` | 最近一次成功重建的 `RebuildSummary`（**无锁**），首次重建前为 `None`；供 dashboard 只读读取已摄取/被跳过的上游 |
| `rebuild_snapshot` | `async (&self) -> Result<RebuildSummary, GatewayError>` | 从注册表**并发**摄取（每个 ingest 受该 handle 的 `call_timeout` 约束）→ 建索引 → 原子换入新快照；经重建锁串行化；返回 `RebuildSummary` 并存为 `last_summary` |

### 函数 `run_rebuild_worker`（`lib.rs`）

| 函数 | 签名 | 说明 |
|------|------|------|
| `run_rebuild_worker` | `async (state: GatewayState, rx: mpsc::Receiver<String>)` | 排空 `rx`、**每波突发合并为一次重建**；channel 关闭（所有 `RebuildTrigger` 发送端 drop）时退出。`serve` spawn 它来处理上游 `tools/list_changed` |

## 依赖

- 内部：`metatools`（`GatewaySnapshot`）、`catalog`（`Catalog`）、`retrieval`（`build_strategy`）、`upstream`
  （`UpstreamRegistry` / `UpstreamHandle`）。
- 外部：`arc-swap`（`ArcSwap` 热替换）、`tokio`（`sync::Mutex` 重建锁、`task::JoinSet` 并发摄取、`mpsc` 触发
  channel）、`thiserror`（`GatewayError`）、`tracing`（重建日志）。

## 关键不变量

- **build-then-swap**：先在临时变量里把新 catalog/策略完全建好，再 `store` 原子换入；切换前的快照对读者始终完整。
- **读无锁**：`snapshot()` 只 `ArcSwap::load_full`，从不触碰重建锁；持有的旧 `Arc<GatewaySnapshot>` 在被换出后
  仍可安全读到生命周期结束。
- **重建经 `Mutex` 串行化**：`rebuild_snapshot` 全程持 `rebuild_lock`，使并发触发不会把陈旧快照留作最终态
  （last-store-wins）。
- **单上游失败/挂起隔离**：每个上游在独立任务里摄取、各自受 `call_timeout` 约束；超时/报错的上游被记入
  `skipped`，**绝不**阻塞其余上游或拖死整次重建（这彻底修复了 B.1 串行摄取里 hung 上游饿死后续重建的隐患）。
- **worker 合并**：`run_rebuild_worker` 把一波连续触发 coalesce 成单次重建，避免突发抖动放大成多次无谓重建。

## 向下导航

- 内部细节见 L3：[gateway](../L3-details/gateway.md)
- 逐文件 API 见 L4：[lib](../L4-api/gateway-lib.md)
- 快照/元工具见：[metatools L2](./metatools.md)
