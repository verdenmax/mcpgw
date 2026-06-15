# M6.T3 设计：审计落库（append-only JSONL）

> 状态：已定稿，待 writing-plans 细化为实施计划。
> 关联：roadmap `M6 — 可观测性/审计`；上游设计 `docs/superpowers/specs/2026-06-14-mcpgw-m6-observability-design.md`（§4 给出 T3 纲要）。
> 前置：M6.T1 已合并（`observe` crate + `CallRecord` + `CallSink` + `TracingSink`；`downstream::GatewayServer::call_tool` 埋点并向 `Arc<[Arc<dyn CallSink>]>` 扇出）。

## 1. 目标与范围

把 M6.T1 已产出的、**仅元数据**的 `observe::CallRecord` **持久化**为 append-only 的 JSONL 审计文件，供事后用 `jq`/`duckdb` 等外部工具检索。新增一个实现 `CallSink` 的 `JsonlSink`，由**后台线程**异步落盘；`[audit]` 开关控制是否装配。

**范围内：** `JsonlSink` + 后台 writer 线程 + `[audit]` 配置 + `mcpgw` 装配/优雅收尾 + 分层文档 + 测试。
**范围外（不做）：** 内置文件轮转/retention（交外部 `logrotate`）、指标（M6.T2）、code-mode（M6.T4）、审计内容加密/签名、远程/网络 sink。

**硬不变量（继承 M6）：**
- **仅元数据**：`JsonlSink` 只序列化 `CallRecord`，其公开类型从类型上就装不下参数/返回内容（只有 `*_bytes` 大小），故审计文件**绝不含 payload**。
- **观测绝不影响调用**：`record()` **同步、非阻塞、绝不 panic、绝不阻塞调用热路径**；落盘故障只降级记 `warn`，工具调用本身不受影响。
- `metatools` 保持纯逻辑，**不**依赖 `observe`。

## 2. 已确认的关键决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 文件轮转/retention | **无内置**：单个 append 文件，交外部 `logrotate`（文档给出 `copytruncate` 方案及其取舍） | 纯 Rust、零额外依赖、最简；审计单文件天然 append-only |
| 写失败/磁盘满降级 | **保活**：限频 `warn` + 丢弃该行，writer **不退出**；I/O 恢复后自动续写 | 瞬时故障（满盘后清理）可自愈；不违反"绝不影响调用" |
| 关闭时收尾 | **有界优雅 drain**：关闭时 drain 队列、`flush` + `fsync`，超时上限兜底 | 干净退出时审计完整可保证；超时防止挂死 |
| 并发实例同文件 | **单进程单 writer**：一条后台线程 + `O_APPEND`；同路径多进程视为误配，文档劝退 | std-only、无锁、无新依赖 |
| 后台 writer 机制 | **专用 OS 线程 + `std::sync::mpsc::sync_channel` + `BufWriter<File>`** | `observe` 保持 std-only（**不**引入 `tokio`）；阻塞 I/O 放独立线程是教科书做法；断连即 drain 对优雅关闭是 FIFO-安全的 |

## 3. 架构与数据流

```
call_tool ──► sink.record(&CallRecord)
                  │  JsonlSink: serde_json::to_string(rec) → SyncSender::try_send(line)
                  │     ├─ Ok            → 入队
                  │     └─ Full/Disconn. → dropped += 1, 限频 warn（绝不阻塞/ panic）
                  ▼
          [bounded sync_channel(AUDIT_CHANNEL_CAPACITY)]
                  ▼
   后台 OS 线程（spawn_writer 内 std::thread::spawn）：
     while let Ok(line) = rx.recv():
        write line + '\n' 到 BufWriter<File>
        while let Ok(more) = rx.try_recv(): write more     // 批量
        flush()                                            // BufWriter → OS（按批）
        // 写失败 → 限频 warn，continue（保活）
     // 所有 Sender drop → recv() 返回 Err（且队列已 FIFO drain 完）
     flush() + file.sync_all()   // 关闭 fsync，退出
```

### 3.1 `observe::audit` 模块（新文件 `crates/observe/src/audit.rs`）

- **`JsonlSink`**（`impl observe::CallSink`）
  - 持 `SyncSender<String>` + `Arc<AtomicU64>`（dropped 计数）。
  - `record(&CallRecord)`：`serde_json::to_string(rec)`（失败极罕见 → 限频 `warn` 跳过）→ `try_send(line)`；`Full`/`Disconnected` → `dropped.fetch_add(1)` + 限频 `warn`（首次 + 2 的幂次时打印累计丢弃数）。
  - **Clone**：克隆共享同一 `SyncSender` 与同一 dropped 计数（供 stdio/http 两个 `GatewayServer` 共享）。
- **`AuditWriter`**（不透明句柄）
  - 持后台线程 `JoinHandle<()>`；`join(self)`：阻塞直到 writer drain + `flush` + `fsync` + 退出。
  - **不持任何 Sender**——drain 的触发是"所有 `JsonlSink` 被 drop"，故句柄与 sink 解耦，避免悬挂 Sender 卡住 drain。
- **`spawn_writer(path: &Path, capacity: usize) -> io::Result<(JsonlSink, AuditWriter)>`**
  - `OpenOptions::new().create(true).append(true).open(path)?`——**打不开即 `Err`**（供 `mcpgw` 启动期 fail-fast）。
  - `sync_channel(capacity)`；`std::thread::spawn` 跑 writer loop（持 `Receiver` + `BufWriter<File>`）。
  - 常量 `pub const AUDIT_CHANNEL_CAPACITY: usize = 1024;`（容量暂为常量，不进配置；如需再加）。
- **可测性**：提供 `pub(crate)`/testkit 构造 `JsonlSink`（在一个**无消费者**的满 channel 上）以确定性地测试丢弃计数路径。

### 3.2 持久化口径

- 一条 `CallRecord` → 一行 `serde_json::to_string` + `\n`。`CallRecord` 已派生 `Serialize`，`Option` 字段 `skip_serializing_if`，枚举 snake_case——审计行与 `TracingSink` 字段语义一致。
- **durability**：运行期仅 `BufWriter::flush`（推到 OS page cache，按批）；`fsync`（`file.sync_all`）仅在**优雅关闭**时做一次。文档明示取舍：干净关闭可保证落盘；进程崩溃可能丢失 OS 缓冲中的尾部若干行。

## 4. 配置（`crates/config`）

新增 `[audit]` 段：

```toml
[audit]
enabled = false                 # 默认关闭
path = "mcpgw-audit.jsonl"      # 审计文件路径
```

- `AuditConfig { enabled: bool, path: String }`，二者均 serde `default`；**省略整个 `[audit]` 段 = 关闭**（`#[serde(default)]` on the field）。
- 默认值：`enabled=false`，`path="mcpgw-audit.jsonl"`。
- 校验：`enabled=true` 时不额外校验 path 内容（打开失败在 `mcpgw` 启动期 fail-fast 暴露）。

## 5. `mcpgw` 装配与收尾（`crates/mcpgw/src/main.rs` `run_serve`）

1. 一如既往构造默认 sinks `vec![Arc::new(observe::TracingSink)]`。
2. `let audit_writer = if cfg.audit.enabled { let (sink, writer) = observe::audit::spawn_writer(Path::new(&cfg.audit.path), observe::audit::AUDIT_CHANNEL_CAPACITY).map_err(|e| ...上下文...)?; sinks.push(Arc::new(sink)); Some(writer) } else { None };`——**fail-fast**（与 `resolve_api_keys` 同位置/同风格，打不开文件即 `Err` 终止启动）。
3. `let sinks: Arc<[Arc<dyn CallSink>]> = sinks.into();` 注入 stdio(`GatewayServer::new`) 与 http(`build_router`)，与 T1 完全一致。
4. `select!` 结束后（stdio/http future drop → 两个 `GatewayServer` 及其 sink 克隆 drop）：
   - **drop 本地 `sinks`**（释放最后的 `JsonlSink` 克隆 → channel 断连）。
   - `if let Some(writer) = audit_writer { tokio::time::timeout(AUDIT_DRAIN_TIMEOUT, tokio::task::spawn_blocking(move || writer.join())).await` → 超时则 `warn!("audit writer drain timed out")` 后继续退出。`}`
   - 常量 `AUDIT_DRAIN_TIMEOUT`（如 5s）。
5. 顺序要求（写进 L3 文档）：必须先让两个 `GatewayServer` 的 sink 克隆 drop（select! 自然达成）、再 drop 本地 `sinks`，writer 才能观察到断连并 drain；超时兜底防止某个悬挂的 http 连接 sink 克隆拖死收尾。

## 6. 运维文档（写入 config L3 或 ops 小节）

- **无内置轮转**：进程不监听 SIGHUP、不重开文件。外部轮转两条路：
  1. `logrotate` + `copytruncate`：无需重启进程；**取舍**——copy 与 truncate 之间写入的行可能丢失（审计场景需知悉）。
  2. 停机轮转：停 `mcpgw` → 移走/压缩审计文件 → 重启（零丢失，但有停机）。
- **单写者**：每个 `mcpgw` 进程需独占自己的审计路径；多进程指向同一文件会交错/损坏，属误配。

## 7. 错误处理与不变量（汇总）

- `record()`：序列化失败 / channel 满 / 断连 → 限频 `warn` + 丢弃，**不 panic、不阻塞**。
- writer 线程：写文件失败 → 限频 `warn` + `continue`（保活、自愈）；仅在所有 Sender drop 后正常退出。
- `spawn_writer`：打开文件失败 → `Err`（启动期 fail-fast；不影响已运行实例，因为只在启动装配时调用）。
- 仅元数据 / `metatools` 纯逻辑：不变。

## 8. 测试

**`observe`（`crates/observe`，生产路径、非 testkit）：**
- `writes_n_records_as_valid_jsonl`：临时目录（`std::env::temp_dir()` + 唯一名，结束清理；不引入 `tempfile` 依赖，除非仓库已有）→ `spawn_writer` → 经 `JsonlSink` 发 N 条 → drop sink → `writer.join()` → 读文件得 N 行，每行 `serde_json::from_str` 成功且键集为预期元数据键。
- `channel_full_increments_dropped_without_blocking`：用 `pub(crate)`/testkit 构造器在一个**无人消费**的 `sync_channel(1)` 上建 `JsonlSink`，发 3 条 → 首条入队、其余 `Full` → dropped == 2，无 panic/阻塞。
- `graceful_drain_flushes_all`：发若干条 → drop sink → join → 文件含全部（与第 1 条同机理，强调 drain 完整性）。
- `spawn_writer_open_failure_returns_err`：path 指向不存在的目录 → `Err`。

**`config`：**
- `[audit]` 往返：给定 TOML 解析出 `enabled/path`；省略 `[audit]` → 默认 `enabled=false` + 默认 path。

**`mcpgw`（装配/集成）：**
- 启用 audit 指向临时文件，经现有 serve 测试 harness 起网关 → 发 1 次元工具调用 → 触发关闭 → 优雅 drain 后断言文件含 ≥1 行合法 JSON（且为元数据）。若集成成本过高，退化为"`run_serve` 装配在 `enabled=true` 下成功建出含 `JsonlSink` 的 sinks 且能优雅收尾"的轻量测试。

## 9. 分层文档（DoD）

- **新建** `docs/L4-api/observe-audit.md`：`JsonlSink` / `AuditWriter` / `spawn_writer` / `AUDIT_CHANNEL_CAPACITY` 逐项 API、限频 warn 与 drain/fsync 语义、仅元数据不变量。
- **更新** `docs/L2-components/observe.md`：职责补"可选 JSONL 审计落盘（后台 OS 线程，std-only）"；接口表加 `JsonlSink`/`spawn_writer`；明确**不**引入 `tokio`（用 `std::thread`）。
- **更新** config L2/L3/L4：`[audit]` 段、默认值、运维轮转说明。
- **更新** `docs/L2-components/mcpgw-cli.md` + `docs/L3-details/mcpgw-cli.md`：`serve` 装配按 `[audit]` 追加 `JsonlSink`（fail-fast 打开）、关闭时有界优雅 drain + drop 顺序要求。
- **更新** `docs/L4-api/mcpgw-main.md`：装配/收尾细节。
- **更新** `docs/L1-overview.md`：审计能力、M6.T3 里程碑小结、测试计数块（按 `cargo test --all-features` 实测）。
- **更新** `docs/README.md`（L4 加 observe-audit、里程碑加 M6.T3）+ roadmap（M6.T3 标 ✅）。

## 10. 实现期需现场确认/可能回退的点

- `spawn_writer` 的临时文件测试：优先 `std::env::temp_dir()` + 唯一名 + 结束清理；若仓库**已**用 `tempfile`，可复用以更稳。
- drain 触发依赖"所有 `JsonlSink` 克隆 drop"。需在 `mcpgw` 收尾处实证 drop 顺序（select! 后两个 server 已 drop、再 drop 本地 sinks），并以 `AUDIT_DRAIN_TIMEOUT` 兜底悬挂的 http 连接 sink 克隆。
- 限频 warn 策略（首次 + 2 的幂次）以代码实测为准，避免日志刷屏又不至于完全静默。
- `JsonlSink` 测试构造器的可见性：优先 `pub(crate)` + crate 内 `tests`；若需放 `tests/` 集成测试，则用 `testkit` feature 暴露（与现有 `CaptureSink` 一致）。
