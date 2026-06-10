# L3 — `gateway` 细节

## `ArcSwap<GatewaySnapshot>` 语义

`GatewayState` 把活快照存为 `Arc<ArcSwap<GatewaySnapshot>>`：

- **构造**：`ArcSwap::from_pointee(snapshot)` 把一个 `GatewaySnapshot` 装入新的 `ArcSwap`（内部即 `Arc<…>`）。
- **读**：`snapshot()` 调 `self.snapshot.load_full() -> Arc<GatewaySnapshot>`，无锁、不阻塞。
- **写**：`rebuild_snapshot` 末尾 `self.snapshot.store(Arc::new(new_snapshot))` 原子替换当前指针。

**旧 Arc 读者安全**：`load_full` 返回的是 `Arc` 克隆。即便随后发生 `store` 把指针换掉，先前拿到 `Arc` 的读者仍持有
旧快照的强引用，可继续安全读到自己用完为止；旧快照在最后一个 `Arc` drop 时才释放。读路径因此无需任何锁，也不会
被重建打断（读到"撕裂"的半成品）。

## `rebuild_snapshot`：ingest → build → swap 流程

```text
let _guard = rebuild_lock.lock().await;        // 串行化（见下）
let mut catalog = Catalog::new();              // 全新空 catalog
for name in registry.server_names():
    if let Some(handle) = registry.get(name):
        if let Err(e) = handle.ingest_into(&mut catalog).await:
            warn!(upstream=name, error=e, "ingest failed; skipping")   // 错误隔离
let mut strat = build_strategy(&self.strategy_name)?;   // 策略未实现 -> Err(String)
strat.index(&catalog);                         // 在临时变量里建好索引
self.snapshot.store(Arc::new(GatewaySnapshot::new(catalog, strat)));   // 原子换入
```

**build-then-swap**：新 catalog 与策略全部在局部变量里建完，最后一步才 `store`。切换前活快照保持旧值且完整；切换是
单条原子指针写。绝不会出现"catalog 已换、索引还没建好"的中间态被读者看到。

## 重建用 `tokio::sync::Mutex` 串行化（防陈旧快照）

`rebuild_lock: Arc<Mutex<()>>` 在 `rebuild_snapshot` 入口处 `.lock().await`，全程持有到函数返回。原因：

- 若两个重建并发跑，各自摄取出不同的 catalog，二者的 `store` 顺序无保证。最后落地的可能是**较早**那次的结果，
  从而把**陈旧**快照留作最终态。
- 用 `Mutex` 串行化后，重建一个接一个执行，**last-store-wins** 语义明确：最后开始的那次重建落地的快照即最终态。
- 选 `tokio::sync::Mutex`（异步锁）而非 `std::sync::Mutex`，因为临界区内有 `.await`（`ingest_into`），异步锁可在
  等待 I/O 时让出执行器而不阻塞线程。
- **读者永不碰这把锁**：`snapshot()` 只走 `ArcSwap`，重建进行中读路径仍然无锁、不被阻塞。

## 单上游失败隔离

循环里某 `handle.ingest_into(&mut catalog).await` 失败时仅 `tracing::warn!` 并 `continue`，不让整次重建失败；其余
上游照常摄取，新快照里仍含它们的工具。这沿用 `upstream` 层"一个挂起/失败上游不拖垮其余"的目标。

⚠️ **已知缺口（B.1 不修，留 M1-B.2）**：错误隔离只覆盖**报错/EOF** 的上游；对**已连接但静默**（hung）的上游无保护。
`ingest_into` 走的是 `list_all_tools()`，而 `UpstreamHandle` 的 `call_timeout` **只包住 `call_tool`、不包 `list_all_tools`**，
所以本步对单个 `ingest_into` 无任何超时——一个 hung 上游会让 `ingest_into` 永久挂起，并因持有 `rebuild_lock` 而饿死后续所有
重建（含 M1-B.2 `list_changed` 触发）。M1-B.2 须给每个 ingest 加超时或改并发 ingest（`join_all`）。

## `strategy_name`

`strategy_name: Arc<str>` 在 `new` 时由字符串建立，每次 `rebuild_snapshot` 都用它 `build_strategy(&self.strategy_name)`
新建一份策略再 `index`。把策略名（而非策略实例）存进状态，使每次重建得到干净的、与新 catalog 匹配的索引；也保持
与 `retrieval::build_strategy(strategy: &str)` 的字符串选择约定一致。

## 不属于 B.1 的部分

`connect_all`（按配置 eager-connect 所有上游、填充注册表）与 `serve`（起下游 MCP server、把元工具暴露给客户端）属
**M1-B.2**，本 crate 当前未实现。B.1 只覆盖快照状态 + 重建。

## 相关

- 接口见 L2：[gateway](../L2-components/gateway.md)
- 逐文件 API 见 L4：[lib](../L4-api/gateway-lib.md)
- 快照与元工具见：[metatools L3](./metatools.md)
