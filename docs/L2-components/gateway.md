# L2 — `gateway` 组件

## 职责

网关的**状态与重建层**（M1-B.1）：持有活的、可原子热替换的 `GatewaySnapshot`（经 `ArcSwap`），外加上游连接
注册表 `UpstreamRegistry`，并负责**从上游重建快照**——把每个上游的工具摄取进新 catalog、建索引、原子换入。
读快照**无锁**；重建经 `Mutex` 串行化。它**不**实现元工具函数本身（复用 `metatools`），也**不**起 MCP server 或
做 eager-connect（那是 M1-B.2 的 `connect_all`/`serve`）。

## 公开接口

### 类型 `GatewayState`（`lib.rs`）
可廉价 `Clone` 的共享网关状态：`ArcSwap` 快照（读无锁）+ 上游注册表 + 策略名 + 重建锁。

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(strategy_name: &str) -> Result<Self, String>` | 建空状态（无上游、空 catalog），用给定策略名（如 `"bm25"`）；策略未实现则返回 `Err` |
| `registry` | `(&self) -> &UpstreamRegistry` | 上游注册表（B.2 的 eager-connect 填充；测试注入 mock handle） |
| `snapshot` | `(&self) -> Arc<GatewaySnapshot>` | 加载当前快照（**无锁**，`load_full`） |
| `rebuild_snapshot` | `async (&self) -> Result<(), String>` | 从注册表摄取→建索引→原子换入新快照；经重建锁串行化 |

## 依赖

- 内部：`metatools`（`GatewaySnapshot`）、`catalog`（`Catalog`）、`retrieval`（`build_strategy`）、`upstream`
  （`UpstreamRegistry` / `UpstreamHandle`）。
- 外部：`arc-swap`（`ArcSwap` 热替换）、`tokio`（`sync::Mutex` 重建锁）、`tracing`（摄取失败 warn）。

## 关键不变量

- **build-then-swap**：先在临时变量里把新 catalog/策略完全建好，再 `store` 原子换入；切换前的快照对读者始终完整。
- **读无锁**：`snapshot()` 只 `ArcSwap::load_full`，从不触碰重建锁；持有的旧 `Arc<GatewaySnapshot>` 在被换出后
  仍可安全读到生命周期结束。
- **重建经 `Mutex` 串行化**：`rebuild_snapshot` 全程持 `rebuild_lock`，使并发触发不会把陈旧快照留作最终态
  （last-store-wins）。
- **单上游失败隔离**：某上游 `ingest_into` 失败仅 `warn!` + skip，其余上游照常进入新快照。

## 向下导航

- 内部细节见 L3：[gateway](../L3-details/gateway.md)
- 逐文件 API 见 L4：[lib](../L4-api/gateway-lib.md)
- 快照/元工具见：[metatools L2](./metatools.md)
