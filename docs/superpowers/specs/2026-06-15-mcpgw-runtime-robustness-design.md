# mcpgw 运行态健壮性整改设计（5 项 Minor 发现）

> 状态：已定稿，待 writing-plans 细化为实施计划。
> 来源：本仓库全量审计的 10 项 Minor 中的 5 项。其余 5 项（HTTP 非环回+空 key、reqwest 版本漂移、gateway-lib 文档措辞、classify/JsonlSink 测试缺口等）仍暂缓。
> 基线（已合并 master `02b3548`）：`cargo test --all-features` 175 passed / 3 ignored，fmt/clippy/build --locked 干净。

## 目标与范围

修复 5 项运行态健壮性/正确性 Minor 问题，每项尽量 TDD（部分为难以单测的防御性改动，将如实说明）。**不**新增任何 crate 依赖。

待修：
- M1 缓存 hash 碰撞键（`retrieval/caching.rs`）
- M2 `ingest` 任务 `expect` panic 破坏崩溃隔离（`gateway/lib.rs`）
- M3 上游 `isError` 结果被记为 `outcome=ok`（`downstream/lib.rs`）
- M4 HTTP 缺 `with_graceful_shutdown`，关闭总等满 5s 且跳过 fsync（`mcpgw/main.rs`）
- M5 `Arc::try_unwrap` 收尾在有 clone 时跳过优雅 cancel（`upstream/connection.rs` + `mcpgw/main.rs`）

不变量：保持纯 Rust、最小依赖；仅元数据审计不变；`metatools` 纯逻辑不变；现有公开行为（检索/路由/鉴权）不回归。

## M1 — 缓存键改为文本 String（消除 hash 碰撞）

**现状**：`GenCache` 的 `current`/`previous` 为 `HashMap<u64, Arc<[f32]>>`，键是文本的 64 位 FNV 哈希；命中只比哈希、不比文本，故两个哈希相同的不同文本会**静默返回错误向量**。

**修复**：把两代 map 的键从 `u64` 改为文本 `String`：
- `current`/`previous`: `HashMap<String, Arc<[f32]>>`；删除 `hash_text`。
- `GenCache::get(&mut self, key: &str)`、`insert(&mut self, key: String, value)`：逻辑不变（current→previous + promote-on-hit；满则轮转），仅键类型改变。
- `embed()`：去重/查找直接用文本（`resolved: HashMap<String, Arc<[f32]>>`、`miss_seen: HashSet<&str>` 或按文本去重）；重组按输入顺序从本地 `resolved` 取（沿用现有「本地 resolved 防止超大批次中途驱逐丢结果」结构）。`Mutex` 仍不跨 `.await` 持有。
- 碰撞从此**结构上不可能**；内存仍有界 ~`2*CAP`（多出每条文本字节，可忽略）。

**测试**：沿用既有 4 个集成测试 + 现有 3 个有界/promote/oversized 单测（把其中用 `hash_text` 造确定性向量的测试 embedder 改为按文本字节派生向量，不再依赖 `hash_text`）。新增：两个不同文本各自缓存、互不串向量（String 键下即天然成立，作为回归护栏）。

## M2 — `ingest` 任务 panic 降级为 skip（恢复崩溃隔离）

**现状**：`rebuild_snapshot` 里 `let (name, outcome, local) = joined.expect("ingest task panicked");`。若某 ingest 任务 panic（如 rmcp 反序列化恶意 `tools/list` 回包），`join_next()` 返回 `Err(JoinError)`，`expect` 再次 panic——初始快照构建期会**崩溃整个启动**，worker 路径会**静默杀死 rebuild worker**，破坏其余代码精心维持的崩溃隔离。

**修复**：把 `joined` 的 `Result<_, JoinError>` 显式 match：
```rust
let (name, outcome, local) = match joined {
    Ok(v) => v,
    Err(e) => {
        tracing::warn!(error = %e, "ingest task panicked/cancelled; skipping it");
        summary.skipped.push(("<ingest task>".to_string(), format!("task failed: {e}")));
        continue;
    }
};
```
（任务 panic 后无法回收其 upstream 名，故记通用 `"<ingest task>"` skip + warn。）单个 ingest panic 不再能崩溃启动或杀死 worker。

**测试**：通过公开 API 触发「客户端侧 ingest 任务 panic」很困难（需 rmcp 在回包上 panic）。本项为防御性改动；以「现有 rebuild 测试不回归 + 该分支逻辑显然正确」为准，并在计划中如实标注测试局限。

## M3 — 上游工具级错误（`isError`）记为 `outcome=Error`

**现状**：`metatools::call_tool` 对任何成功往返（含上游 `is_error=true` 的结果）返回 `Ok(result)`；`downstream::call_tool` 的 `Ok(result)` 臂一律记 `outcome=Ok / error_kind=None`，转发给客户端的 `CallToolResult`（含 `isError`）正确，但**审计/追踪低估了工具级失败**。

**修复**：在 `Ok(result)` 臂按 `result.is_error` 分类（结果仍原样转发）：
```rust
Ok(result) => {
    let (outcome, kind) = if result.is_error == Some(true) {
        (CallOutcome::Error, Some("upstream_tool_error"))
    } else {
        (CallOutcome::Ok, None)
    };
    (Ok(result), MetaTool::CallTool, Some(name.to_string()), outcome, kind)
}
```
`error_kind` 新增取值 `"upstream_tool_error"`，纳入既有 taxonomy。

**测试**：在 testkit `MockUpstream` 加一个总是返回 `CallToolResult::error(...)`（`is_error=Some(true)`）的 `fail` 工具；在 `downstream/tests/server.rs` 用 `CaptureSink` 经网关调 `mock__fail`，断言：转发结果 `is_error==Some(true)` 且记录 `outcome=Error`、`error_kind=Some("upstream_tool_error")`、`target_tool=Some("mock__fail")`。

## M4 — HTTP 优雅关闭（`with_graceful_shutdown`）

**现状**：HTTP 分支是 `axum::serve(listener, router).await`，无优雅关闭。ctrl-c/stdio-EOF 时 `select!` 只 drop 掉 accept future；已接受的 keep-alive 连接任务被遗弃、仍各持一份 `GatewayServer`→`Arc<JsonlSink>` clone，于是审计 channel 不断连，关闭**总是**等满 `AUDIT_DRAIN_TIMEOUT`（5s）且**跳过最终 fsync**。

**修复**：把 HTTP 服务改为带优雅关闭的**后台任务**，由 std `tokio::sync::oneshot` 驱动（无新依赖）：
- `let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();`
- HTTP 任务（仅 http_enabled 时）：`tokio::spawn(axum::serve(listener, router).with_graceful_shutdown(async move { let _ = shutdown_rx.await; }))`。
- 前台 `select!` 只等**关闭触发**：stdio 客户端断开、`ctrl_c`、以及（HTTP-only 模式）HTTP 任务自行结束（出错）。任一触发即得 `outcome`。
- 触发后：`let _ = shutdown_tx.send(());`（点燃优雅关闭）→ `tokio::time::timeout(HTTP_SHUTDOWN_TIMEOUT, http_task)` 有界等待 HTTP 任务排空。排空后**每会话 sink clone 被释放**，紧随其后的 `drop(sinks)` + 审计 drain 立即完成（不再等满 5s，且能 fsync）。
- 新常量 `HTTP_SHUTDOWN_TIMEOUT`（如 5s）。
- 三种传输模式都要正确：stdio-only（无 HTTP 任务）、http-only（前台还需在 HTTP 任务结束时收尾）、stdio+http。

**收尾顺序**（写入文档）：`select!` 触发 → `shutdown_tx.send` → 有界 await HTTP 任务 → `drop(sinks)` → 审计 drain → 上游 cancel。

**测试**：用现有 serve harness（`mcpgw/tests/`，rmcp `StreamableHttpClient` over child process 或进程内）起 http-only + `[audit]`，发一次调用，触发关闭，断言：审计文件含该调用且进程**及时**退出（不卡满超时）。若 e2e 偏重，退化为「装配/收尾在 http_enabled 下成功、HTTP 任务收到关闭信号即退出」的较轻测试。

## M5 — 上游收尾改用非消费式 cancel

**现状**：`mcpgw` 收尾 `if let Ok(h) = Arc::try_unwrap(handle) { h.shutdown().await; }`。`shutdown(self)` 消费 `self` 调 `client.cancel().await`，故需独占所有权；但 rebuild worker（进程级）与在飞 `call_tool` 常持 clone → `try_unwrap` 失败 → **跳过优雅 cancel**，退化为 drop-cancel。

**修复**：利用 rmcp `RunningService::cancellation_token(&self) -> RunningServiceCancellationToken`（非消费式）。
- `UpstreamHandle` 新增 `pub fn cancel(&self) { self.client.cancellation_token().cancel(); }`（`&self`，触发服务取消令牌，fire-and-forget）。
- 保留 `shutdown(self).await`（消费式、等待 quit）。
- 收尾循环：
  ```rust
  if let Some(handle) = state.registry().remove(&name) {
      match Arc::try_unwrap(handle) {
          Ok(h) => h.shutdown().await,   // 独占：优雅 + 等待
          Err(shared) => shared.cancel(), // 仍有 clone：经令牌取消，不再静默跳过
      }
  }
  ```
- 每个上游至少都会收到 cancel 信号；DropGuard 仍在最后一个 Arc drop 时兜底回收子进程。

**测试**：子进程回收难以单测；以「`cancel(&self)` 编译/调用不 panic、收尾对仍有 clone 的 handle 也调用 cancel」为准；如可行，加一个「收尾循环在有 clone 时走 `Err(shared) => cancel()` 分支」的逻辑测试。如实标注局限。

## 分层文档（DoD）

- `docs/L3-details/downstream.md` / `docs/L4-api/downstream-lib.md`：`error_kind` taxonomy 增加 `upstream_tool_error`（上游工具级 `isError`）。
- `docs/L3-details/retrieval.md` / `docs/L4-api/retrieval-embedder.md`：缓存键由「64 位内容哈希」更新为「文本 `String`（无碰撞）」；删除哈希碰撞注脚。
- `docs/L3-details/gateway.md`：rebuild ingest 任务 panic 现降级为 skip+warn（不再崩溃）。
- `docs/L3-details/mcpgw-cli.md` / `docs/L4-api/mcpgw-main.md`：HTTP 现经 `with_graceful_shutdown`（oneshot 驱动）+ 有界 `HTTP_SHUTDOWN_TIMEOUT` 排空，关闭顺序更新；上游收尾改为「独占→shutdown().await，否则→cancel()」。
- `docs/L4-api/upstream-connection.md`：`UpstreamHandle::cancel(&self)` 新增；`shutdown(self)` 仍在。
- 若测试计数变化，更新 `docs/L1-overview.md` 测试计数块（按 `cargo test --all-features` 实测）。

## 错误处理与不变量（汇总）

- M2/M4/M5 都不引入 panic 路径；M2 把 panic 降级为 skip，M4/M5 让关闭/取消更确定。
- M1 不改 `Embedder` 对外契约（等长、按输入顺序）。
- M3 只改观测分类，转发结果与协议不变。
- 不新增 crate 依赖；不触碰其余 5 项 Minor。

## 实现期需现场确认/可能回退的点

- M4 前台 `select!` 同时处理三模式：`Option<JoinHandle>` 在 `select!` 分支里的 `&mut` await + 关闭后 `timeout` 二次 await 需写法谨慎；HTTP-only 模式需有「HTTP 任务结束即收尾」的分支（否则无触发源）。
- M5：`cancellation_token().cancel()` 为 fire-and-forget；确认其确能让服务循环退出并最终回收子进程（与既有 DropGuard 兜底一致）。`shutdown(self).await` 仍用于独占路径以等待 quit。
- M3：`error_kind` 新值 `"upstream_tool_error"` 需在 L3/L4 taxonomy 表同步；`fail` mock 工具加在 `testkit`（feature-gated，不影响生产）。
- M1：测试 embedder 不再用 `hash_text`，改按文本字节派生确定性向量；其余缓存测试逻辑不变。
- M2：如确实无法构造 panic 注入测试，明确以防御性改动 + 不回归为验收。
