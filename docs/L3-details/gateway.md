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
let mut names_by_id = HashMap::<task::Id, String>::new();   // 任务 Id → 上游名（用于 panic 归因）
for name in registry.server_names():               // 每个上游一个并发任务
    if disabled.is_upstream_disabled(name): continue   // ① 禁用上游：连任务都不 spawn（不发 tools/list）
    if let Some(handle) = registry.get(name):
        let timeout = handle.call_timeout();
        let task_name = name.clone();                // 在 move 进闭包前克隆，供 panic 归因
        let abort = set.spawn(async move {
            let mut local = Catalog::new();          // 任务私有 catalog
            let outcome = tokio::time::timeout(timeout, handle.ingest_into(&mut local)).await;
            (name, outcome, local)
        })
        names_by_id.insert(abort.id(), task_name);
let mut summary = RebuildSummary::default();
let mut catalog = Catalog::new();
while let Some(joined) = set.join_next().await:
    // 任务 panic/取消（JoinError）→ resolve_joined 按 task::Id 归因后降级为 skipped + warn，
    // 返回 None 则 continue 跳过；绝不 re-panic（保住启动期与重建 worker 的崩溃隔离）。
    let Some((name, outcome, local)) = resolve_joined(joined, &names_by_id, &mut summary) else:
        continue
    match outcome:
        Err(_elapsed)  => summary.skipped.push((name, "ingest timed out"))   // per-ingest 超时
        Ok(Err(e))     => summary.skipped.push((name, e.to_string()))        // 调用错误
        Ok(Ok(_dupes)) => { for tool in local.iter() {
                                if disabled.is_tool_disabled(tool.qualified_name()): continue  // ② 禁用单工具：跳过 upsert
                                catalog.upsert(tool) }; summary.ingested.push(name) }
summary.{ingested,skipped}.sort();                 // 结果确定、可断言
let mut strat = build_strategy(&self.strategy_name, &self.backends)?;  // 未知名/缺 embedder|chat -> GatewayError::Strategy
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
- **ingest 任务 panic/取消也被隔离（audit M2）**：`join_next` 拿到的 `JoinError`（任务 panic 或被取消）经私有
  `resolve_joined` 降级为一个 `skipped` 上游（reason `"task failed: …"`）+ 一条 `warn`，并按 `task::Id`→名映射归因到
  其上游（映射缺失时回退为 `"<ingest task>"`），随后 `continue` 跳过该任务——**绝不**把 panic 重新抛出。这把崩溃隔离
  同时贯彻到**启动期**初次构建（`prepare_state`）与**重建 worker**：单个 ingest 任务炸掉不再 `expect`-崩溃整个进程或杀死 worker。
- 其余上游照常摄取，新快照里仍含它们的工具。这把 `upstream` 层"一个挂起/失败上游不拖垮其余"的目标贯彻到了摄取期。

## 重建用 `tokio::sync::Mutex` 串行化（防陈旧快照）

`rebuild_lock: Arc<Mutex<()>>` 在 `rebuild_snapshot` 入口处 `.lock().await`，全程持有到函数返回。原因：

- 若两个重建并发跑，各自摄取出不同的 catalog，二者的 `store` 顺序无保证。最后落地的可能是**较早**那次的结果，
  从而把**陈旧**快照留作最终态。
- 用 `Mutex` 串行化后，重建一个接一个执行，**last-store-wins** 语义明确：最后开始的那次重建落地的快照即最终态。
- 选 `tokio::sync::Mutex`（异步锁）而非 `std::sync::Mutex`，因为临界区内有 `.await`（并发 ingest 的 join），异步
  锁可在等待 I/O 时让出执行器而不阻塞线程。
- **读者永不碰这把锁**：`snapshot()` 只走 `ArcSwap`，重建进行中读路径仍然无锁、不被阻塞。

## 运行时禁用集 `DisableSet`（子系统 B）：隐藏式过滤 + 持久化

`GatewayState.disabled: Arc<DisableSet>`（默认空集 → 行为与引入前完全一致）是运行时**临时禁用**的唯一真相源。
它只在上面 `rebuild_snapshot` 的**两个过滤点**被读：

- **① 禁用上游**（`is_upstream_disabled(name)`，spawn 前）：被禁用的上游 namespace 连 ingest 任务都不 spawn——
  既不发 `tools/list`、也不进 `summary.ingested`/`skipped`（彻底"隐身"，零网络接触）。
- **② 禁用单工具**（`is_tool_disabled(tool.qualified_name())`，upsert 前）：上游照常摄取，仅把被禁用的那个
  qualified 工具名跳过 `upsert`，其同上游兄弟工具不受影响。

被滤掉的项不进新快照，对下游即 `ToolNotFound`（**隐藏式语义**——`metatools`/`downstream` 读路径零改动）。

**check-then-call 竞态（可接受）**：禁用只在下一次 rebuild 生效。若某调用在禁用变更**落地前**已通过 `find` 拿到名字
并发起 `call_tool`，它可能在禁用后**再成功完成一次**；下次 rebuild 后该工具彻底消失。这是无锁读路径的固有取舍，
不泄漏任何状态，文档与 dashboard 均以一句话明示。

**持久化（可选、best-effort、自愈）**：`DisableSet` 内部为 `RwLock<{upstreams, tools}: BTreeSet<String>>`（有序）+
可选 `path`：

- `load_or_new(path)`：`None` → 纯内存；有 path 且文件在 → 读入；**缺文件 → 空集（正常，非错误）**；
  **坏 JSON / 坏 UTF-8 → 空集 + `warn!`**（自愈，绝不 panic、绝不挡启动）。读不出的陈旧名也不会卡死系统。
- 每次 `disable_*`/`enable_*` 返回 `changed: bool`，**仅 changed 时**才写盘：序列化 `DisabledSnapshot` →
  写临时文件 → `flush` + `fsync` → **原子 `rename`** 覆盖目标。写失败只 `warn!`，**内存集仍是权威**（best-effort：
  持久化丢失最多让重启后少了几个禁用项，绝不影响运行时正确性）。
- 内存集是单一真相源；磁盘只是重启恢复用的镜像。陈旧（已不存在的上游/工具）名被保留，过滤时永不命中、可由 `enable` 清除。

**装配点**：`main.rs` 在**首次 rebuild 之前**用 `with_disabled(DisableSet::load_or_new(disabled_state_path))` 注入，
**且独立于 `dashboard.enabled`**——即使不开 dashboard，已持久化的禁用项也从启动起生效。dashboard 的 admin API 经
`disabled_arc()` 拿到同一 `Arc<DisableSet>` 做读写（详见 [dashboard L3](./dashboard.md)）。

## 上游热重载 `reconcile_upstreams`（子系统 C）：纯三向 diff + best-effort + 与 M4 组合

dashboard 的在线改配（子系统 C）落盘新 `mcpgw.toml` 后，对 `[[upstream]]` 的增/删/改做**秒级热重载**——无需重启进程。
协调逻辑落在 `gateway`（已依赖 `upstream`、拥有 `registry` + `rebuild_snapshot`，复用现成能力、不引入 dashboard→main 依赖），
拆成「纯函数算计划 + apply 复用既有路径」两段：

**① 纯三向 diff（`plan_upstream_reconcile(old, new) -> ReconcilePlan`，重点单测）**：按上游 `name` 分类——

```text
removed   = old.name − new.name              → 计划 registry.remove(name)（丢弃 handle = 断连）
added     = new.name − old.name              ┐ 入 to_connect
changed   = 同名但 UpstreamConfig != 旧       ┘ 入 to_connect（整体 == 判定：任一字段变即重连，最简无歧义）
unchanged = 同名且 config 相等                → 不入任何集合，**连接零中断**
```

`changed` 用整体 `UpstreamConfig`（派生 `PartialEq`）相等判定，任一字段变化即整体 reconnect。

**② apply（`reconcile_upstreams(old, new, trigger)` 方法）**：

```text
let plan = plan_upstream_reconcile(old, new);
if plan.removed.is_empty() && plan.to_connect.is_empty():
    return ReconcileSummary::default()         // 纯 no-op：跳过 remove/connect/rebuild（零开销）
for name in plan.removed: self.registry.remove(name)          // 断连被删上游
if !plan.to_connect.is_empty():
    let csum = upstream::connect::connect_all(self.registry(), &plan.to_connect, trigger).await
    connect_failures = csum.skipped            // 复用 eager-connect：逐个连、成功 insert/替换、失败记 skipped
let _ = self.rebuild_snapshot().await          // 单次重建：新工具集生效，同时应用 M4 禁用过滤
ReconcileSummary{ added, removed, reconnected, connect_failures }
```

- **best-effort（与 `connect_all`/启动期一致）**：某上游连接失败 → 记 `connect_failures`、**不**中止其余、**不**回滚已落盘的新配置
  （落盘 = 用户意图）；该上游本次缺席，修好后再 PUT 或重启即恢复。dashboard 据此把 `applied_upstreams` 基线设为**已成功连上者**
  （排除失败者），故对同一配置再 PUT 会重试仍失败的上游，而非误判为"unchanged"。
- **`added`/`reconnected` 是意图、`connect_failures` 才是真相**：`ReconcileSummary` 里 `added`/`reconnected` 记的是计划尝试，
  与 `connect_failures` 交叉看才是真正生效的连接（changed 上游若重连失败，计入 `connect_failures`、旧连接仍保留——`connect_all` 仅成功才 insert/替换、失败只 warn 记 skipped，reconcile 又只对 `plan.removed` 调 remove，故 changed 上游绝不被摘除）。
- **与 M4 禁用过滤组合**：reconcile 末尾主动 `rebuild_snapshot`，而 rebuild 照常读 `DisableSet` 过滤——若新加/改的上游仍在禁用集里，
  rebuild 照样跳过它（隐藏保持）。两特性天然正交：一个决定"连哪些上游"，一个决定"快照里露哪些工具"。

**调用点**：dashboard 的 `put_config` handler 在原子落盘后调
`state.gateway.reconcile_upstreams(applied_upstreams, new_cfg.upstreams, rebuild_trigger)`（详见 [dashboard L3](./dashboard.md) 的
「在线改配子系统」）。

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

`strategy_name: Arc<str>` 在 `new`/`with_embedder`/`with_backends` 时由字符串建立，每次 `rebuild_snapshot` 都用它
`build_strategy(&self.strategy_name, &self.backends)` 新建一份策略再 `index`。把策略名（而非策略实例）存进状态，
使每次重建得到干净的、与新 catalog 匹配的索引；也保持与 `retrieval::build_strategy(name, &Backends)` 的字符串选择约定一致。

策略工厂按 **name + backends** 构建：`"bm25"` 无需后端；`"vector"`/`"hybrid"` 需要 `backends.embedder`，缺则返回
`StrategyError::EmbedderRequired`；`"subagent"` 需要 `backends.chat`，缺则 `StrategyError::ChatModelRequired`；其余名字
`NotImplemented`。`backends: Backends`（`embedder`/`chat`/`subagent_candidates`）由 `with_embedder`/`with_backends` 持有进 state，
rebuild 时**复用同一份** backends——若 `embedder` 是 `CachingEmbedder`，其缓存便跨 rebuild 保留，只对新增工具计算嵌入。
`new`（`Backends::default()`，无后端）只能构建 bm25。

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
