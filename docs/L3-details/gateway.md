# L3 — `gateway` 细节

## `ArcSwap<GatewaySnapshot>` 语义

`GatewayState` 把活快照存为 `Arc<ArcSwap<GatewaySnapshot>>`：

- **构造**：`ArcSwap::from_pointee(snapshot)` 把一个 `GatewaySnapshot` 装入新的 `ArcSwap`（内部即 `Arc<…>`）。
- **读**：`snapshot()` 调 `self.snapshot.load_full() -> Arc<GatewaySnapshot>`，无锁、不阻塞。
- **写**：`rebuild_snapshot` 末尾 `self.snapshot.store(Arc::new(new_snapshot))` 原子替换当前指针。

**旧 Arc 读者安全**：`load_full` 返回的是 `Arc` 克隆。即便随后发生 `store` 把指针换掉，先前拿到 `Arc` 的读者仍持有
旧快照的强引用，可继续安全读到自己用完为止；旧快照在最后一个 `Arc` drop 时才释放。读路径因此无需任何锁，也不会
被重建打断（读到"撕裂"的半成品）。

## `rebuild_snapshot`：并发 ingest → build → swap 流程

```text
let _guard = rebuild_lock.lock().await;            // 串行化（见下）
let mut set = JoinSet::new();
for name in registry.server_names():               // 每个上游一个并发任务
    if let Some(handle) = registry.get(name):
        let timeout = handle.call_timeout();
        set.spawn(async move {
            let mut local = Catalog::new();          // 任务私有 catalog
            let outcome = tokio::time::timeout(timeout, handle.ingest_into(&mut local)).await;
            (name, outcome, local)
        })
let mut summary = RebuildSummary::default();
let mut catalog = Catalog::new();
while let Some((name, outcome, local)) = set.join_next().await:
    match outcome:
        Err(_elapsed)  => summary.skipped.push((name, "ingest timed out"))   // per-ingest 超时
        Ok(Err(e))     => summary.skipped.push((name, e.to_string()))        // 调用错误
        Ok(Ok(_dupes)) => { for tool in local.iter() { catalog.upsert(tool) }; summary.ingested.push(name) }
summary.{ingested,skipped}.sort();                 // 结果确定、可断言
let mut strat = build_strategy(&self.strategy_name, self.embedder.as_ref())?;  // 未实现/缺 embedder -> GatewayError::Strategy
strat.index(&catalog);
self.snapshot.store(Arc::new(GatewaySnapshot::new(catalog, strat)));   // 原子换入
```

**并发摄取**：每个上游在 `JoinSet` 的独立任务里摄取进**任务私有** `local` catalog；主循环 `join_next` 收集结果，
仅把成功者的工具 `upsert` 进最终 catalog（次序无关，因 `name` 排序使结果确定）。

**per-ingest 超时**：每个 `ingest_into` 被 `tokio::time::timeout(handle.call_timeout(), …)` 包住。

**build-then-swap**：新 catalog 与策略全部在局部变量里建完，最后一步才 `store`。切换前活快照保持旧值且完整；切换是
单条原子指针写。绝不会出现"catalog 已换、索引还没建好"的中间态被读者看到。

## 单上游失败 / 挂起隔离（已修复 B.1 的死锁隐患）

B.1 曾串行 `ingest_into` 且无超时——一个**已连接但静默（hung）**的上游会让 `ingest_into` 永久挂起，并因持有
`rebuild_lock` 而饿死后续所有重建。B.2 通过**并发 ingest + per-ingest 超时**彻底修复：

- 每个上游独立任务、独立超时；超时者映射为 `skipped`（`"ingest timed out"`），报错者映射为 `skipped`（错误文本）。
- 一个 hung/慢/报错的上游**只**进入 `skipped`，既不阻塞其余上游的摄取，也不拖住整次重建（因而不饿死后续重建）。
- 其余上游照常摄取，新快照里仍含它们的工具。这把 `upstream` 层"一个挂起/失败上游不拖垮其余"的目标贯彻到了摄取期。

## 重建用 `tokio::sync::Mutex` 串行化（防陈旧快照）

`rebuild_lock: Arc<Mutex<()>>` 在 `rebuild_snapshot` 入口处 `.lock().await`，全程持有到函数返回。原因：

- 若两个重建并发跑，各自摄取出不同的 catalog，二者的 `store` 顺序无保证。最后落地的可能是**较早**那次的结果，
  从而把**陈旧**快照留作最终态。
- 用 `Mutex` 串行化后，重建一个接一个执行，**last-store-wins** 语义明确：最后开始的那次重建落地的快照即最终态。
- 选 `tokio::sync::Mutex`（异步锁）而非 `std::sync::Mutex`，因为临界区内有 `.await`（并发 ingest 的 join），异步
  锁可在等待 I/O 时让出执行器而不阻塞线程。
- **读者永不碰这把锁**：`snapshot()` 只走 `ArcSwap`，重建进行中读路径仍然无锁、不被阻塞。

## `run_rebuild_worker`：合并突发触发

```text
while rx.recv().await.is_some():           // 阻塞等下一个触发
    while rx.try_recv().is_ok() {}         // 排空已积压的触发（coalesce 一波突发）
    state.rebuild_snapshot().await -> info!/warn! 记录 RebuildSummary
```

`serve` spawn 一个 `run_rebuild_worker(state, rx)`，`rx` 是 `RebuildTrigger`（`mpsc::Sender<String>`）的接收端。
每收到一个触发就先排空 channel 里其余已积压的触发，**把一波连续的 list_changed 合并成单次重建**——多个上游同时变更、
或一个上游短时间内多次 `tools/list_changed` 都只触发一次重建，避免抖动放大。channel 关闭（所有发送端 drop）时
`recv` 返回 `None`，worker 退出。

## `strategy_name` 与 `embedder`：策略工厂

`strategy_name: Arc<str>` 在 `new`/`with_embedder` 时由字符串建立，每次 `rebuild_snapshot` 都用它
`build_strategy(&self.strategy_name, self.embedder.as_ref())` 新建一份策略再 `index`。把策略名（而非策略实例）存进状态，
使每次重建得到干净的、与新 catalog 匹配的索引；也保持与 `retrieval::build_strategy(name, embedder)` 的字符串选择约定一致。

策略工厂按 **name + embedder** 构建：`"bm25"` 无需 embedder；`"vector"` 需要 embedder，缺则返回
`StrategyError::EmbedderRequired`（经 `with_embedder` 注入）；`"hybrid"` 同样需要 embedder（缺则 `EmbedderRequired`）；其余名字
`NotImplemented`。`embedder: Option<Arc<dyn Embedder>>` 由 `with_embedder` 持有进 state，rebuild 时**复用同一个** embedder
实例——若它是 `CachingEmbedder`，其缓存便跨 rebuild 保留，只对新增工具计算嵌入。`new`（embedder 为 `None`）只能构建 bm25。

## 错误类型 `GatewayError`

`new` / `rebuild_snapshot` 返回 `Result<_, GatewayError>`（取代早期裸 `String`）。当前仅
`Strategy(String)`（检索策略未实现/构建失败，来自 `build_strategy`）。摄取期的失败**不**走 `GatewayError`——它们
被收进 `RebuildSummary.skipped`，因为单上游失败不应让整次重建失败。

## M1-B.2 的接入：`serve` 装配

`mcpgw serve` 把本 crate 与 `upstream::connect` / `downstream` 装配成活网关：`connect_all` eager-connect 上游并
把 `RebuildTrigger` 接进来 → 初始 `rebuild_snapshot` → spawn `run_rebuild_worker` 处理上游 `list_changed` →
`downstream::GatewayServer` over stdio。详见 [mcpgw-cli L3](./mcpgw-cli.md) 与 [downstream L3](./downstream.md)。

## 相关

- 接口见 L2：[gateway](../L2-components/gateway.md)
- 逐文件 API 见 L4：[lib](../L4-api/gateway-lib.md)
- 快照与元工具见：[metatools L3](./metatools.md)
