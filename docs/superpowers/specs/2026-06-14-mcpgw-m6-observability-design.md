# M6 设计：可观测性 / 审计（T1 调用日志/追踪 → T3 审计落库）

> 状态：已通过 brainstorm 评审，待 writing-plans 为 T1 细化为实现计划。
> 前置：M0 / M1 / M2-A / M2-B / M2.T5 已合并到 master（HEAD `976f9bc`）。
> 关联里程碑：roadmap `M6 — 可观测性/审计 + Programmatic Tool Calling`。

## 0. 这是什么 / 程序级分解

M6 是**里程碑级**的，含多个相对独立的子项目。本 spec 把它分解，并**详设第一块（T1）**、给出**第二块（T3）的纲要**；
后续子项目各自走 spec → 计划 → 实现。

| 子项目 | 内容 | 本次范围 | 依赖 |
|--------|------|----------|------|
| **T1** | 结构化调用日志 + 追踪（埋点地基） | ✅ 本次详设 + 实现 | — |
| **T3** | 审计落库（append-only JSONL） | ✅ 本次纲要，T1 后实现 | T1 的埋点与 `CallRecord` |
| **T2** | 用量指标导出（Prometheus `/metrics`） | ⏸ **延后**（sink 架构已留口子） | T1 |
| **T4** | code-mode / 沙箱内 programmatic tool calling | ⏸ **延后**（独立大件，单独立项） | — |

**本次 effort = T1 → T3。** T2/T4 延后，但 T1 的 sink 架构为 T2 留好扩展位。

## 1. 决策记录（brainstorm 已敲定）

| 决策点 | 结论 | 理由 |
|--------|------|------|
| T4 code-mode | **延后**，单独里程碑 | 沙箱+codegen+执行隔离，与可观测性不同维度，路线图标「大、可选」|
| 本次范围 | **T1 → T3**，T2 延后 | T1 是地基；T3 审计贴合治理/安全主线、喂将来 M5；T2 运维向、按需再加 |
| 数据粒度 | **仅元数据，不记 payload** | 契合「密钥永不入日志」安全基线；参数/返回只记**大小** |
| T3 存储 | **append-only JSONL** | 纯 Rust、仅 serde_json、零重依赖；append-only 天然适合审计；外部 jq/duckdb 查 |
| 埋点位置 | **`downstream` 元工具边界** | 面向客户端的咽喉；`metatools` 保持纯逻辑 |
| sink 调用 | **同步非阻塞** | 不拖慢调用热路径；JSONL 经后台 writer 异步落盘 |

## 2. 核心架构：一个埋点，多个 sink

新建小 crate **`observe`**（纯逻辑：serde + tracing；**无 HTTP、无存储重依赖**）：

```
            downstream::GatewayServer（元工具边界）
              │ 计时一次元工具调用，构造 CallRecord（纯元数据）
              ▼
        for sink in sinks { sink.record(&rec) }   // 同步、非阻塞
              ├──────────────► TracingSink（T1）  → 结构化 tracing::info!
              └──────────────► JsonlSink（T3）    → try_send 到后台 writer → append JSONL 文件
```

- 默认 `sinks = [TracingSink]`（T1 恒开，等于把现有 tracing 升级成结构化调用日志）。
- `[audit].enabled` 时 `sinks += JsonlSink`（T3）。
- 将来 T2 = 再加一个 `MetricsSink`，无需改埋点。

### crate 划分

| crate | 新增/改动 | 子项目 |
|------|-----------|--------|
| `observe`（**新**）| `CallRecord` / `MetaTool` / `CallOutcome` / `CallSink` trait / `TracingSink` / `JsonlSink`（+后台 writer）| T1/T3 |
| `downstream` | `GatewayServer` 持有 `Vec<Arc<dyn CallSink>>`；元工具处理体计时 + 构造 `CallRecord` + fan-out；`MetaError` 分类 `error_kind` | T1 |
| `config` | `[audit]` 段（`enabled` / `path`） | T3 |
| `mcpgw` | 据 `[audit]` 装配 sinks（默认 `TracingSink`，开启时 + `JsonlSink`）注入 `GatewayServer` | T3 |
| `Cargo.toml`(ws) | members 加 `crates/observe` | T1 |

> `observe` 依赖 `serde`/`serde_json`/`tracing`，并为 `JsonlSink` 后台 writer 用 `tokio`（已是工作区依赖）。`metatools` **不**依赖 `observe`（保持纯逻辑）。

## 3. T1 详设（本次实现）

### 3.1 `observe` 的公开类型

```rust
/// 哪个元工具被调用。
pub enum MetaTool { SearchTools, GetToolDetails, CallTool }

/// 一次元工具调用的结局。
pub enum CallOutcome { Ok, Error, Timeout }

/// 一次元工具调用的**纯元数据**记录（绝不含参数/返回的内容，只含大小）。
pub struct CallRecord {
    pub ts_unix_ms: u64,
    pub meta_tool: MetaTool,
    /// 仅 CallTool：被转发的上游工具限定名 `{server}__{tool}`。
    pub target_tool: Option<String>,
    /// 仅 CallTool：上游命名空间（从 target_tool 的 `{server}__` 切出）。
    pub upstream: Option<String>,
    pub latency_ms: u64,
    pub outcome: CallOutcome,
    /// 有限集：timeout / upstream_call / tool_not_found / upstream_unavailable /
    /// invalid_params / internal。None 表示成功。
    pub error_kind: Option<&'static str>,
    pub arg_bytes: usize,
    pub result_bytes: usize,
}

/// 调用观测 sink：同步、非阻塞、不得 panic。
pub trait CallSink: Send + Sync {
    fn record(&self, rec: &CallRecord);
}
```

- `CallRecord` 派生 `Serialize`（JSONL 用）；`MetaTool`/`CallOutcome` 用 `serde(rename_all="snake_case")` 序列化成短字符串。
- **不可序列化任何参数/返回内容**——只有 `arg_bytes`/`result_bytes`（对入参 map、出参 result 的序列化字节数）。

### 3.2 `TracingSink`（T1）

```rust
pub struct TracingSink;
impl CallSink for TracingSink {
    fn record(&self, r: &CallRecord) {
        tracing::info!(
            meta_tool = ?r.meta_tool, target_tool = r.target_tool.as_deref(),
            upstream = r.upstream.as_deref(), latency_ms = r.latency_ms,
            outcome = ?r.outcome, error_kind = r.error_kind,
            arg_bytes = r.arg_bytes, result_bytes = r.result_bytes,
            "tool_call"
        );
    }
}
```
复用 mcpgw 已初始化的 `tracing_subscriber::fmt()`（EnvFilter 默认 info）。结构化字段，便于 grep/JSON 化。

### 3.3 埋点：`downstream::GatewayServer::call_tool`

- 用 `std::time::Instant` 包住元工具分发；三臂各自计算 `meta_tool`/`target_tool`/`upstream`/`outcome`/`error_kind`/`*_bytes`。
- `metatools::call_tool` 仍返回 `Result<CallToolResult, MetaError>`；downstream 在把 `Err(e)` 映射成 `CallToolResult::error` **之前**先按 `MetaError` 变体分类 `error_kind`：

  | 路径 | outcome | error_kind |
  |------|---------|------------|
  | 成功 | Ok | None |
  | `MetaError::Timeout` | Timeout | `timeout` |
  | `MetaError::Call(..)` | Error | `upstream_call` |
  | `MetaError::ToolNotFound`（call / get_details「无此工具」）| Error | `tool_not_found` |
  | `MetaError::UpstreamUnavailable` | Error | `upstream_unavailable` |
  | call 参数缺 `name`（现有 `CallToolResult::error` 早退） | Error | `invalid_params` |
  | search/get_details 结果序列化失败（罕见） | Error | `internal` |

  > **未知元工具名**走现有 `McpError::invalid_params`（协议层拒绝，发生在任何网关工作之前）——**不计入 `CallRecord`**。
  > 故 `MetaTool` 仅 3 个变体，无需 `Option`/哨兵。

- `upstream` = `target_tool` 按首个 `__` 切出的前缀（仅 CallTool）。
- 处理体结束时（无论 Ok/Err）`for s in &self.sinks { s.record(&rec) }`。
- `GatewayServer::new` 增加 sinks 参数（或建造者）；现有 `new(state, default_top_k)` 调用点更新（downstream stdio/http 装配 + 测试）。

### 3.4 测试（T1）

- **`observe` 单元**：`CallRecord` 序列化为预期 JSON（字段名/枚举短串）；一个 `Vec<CallRecord>` 捕获型 `CallSink`（测试用）。
- **关键安全测试**：构造一次带「密钥样」参数的 `CallRecord`（其实记录里只有 `arg_bytes`），断言序列化串里**不含**任何明文参数/返回——把「仅元数据」钉死。
- **downstream 集成**：往 `GatewayServer` 注入捕获 sink，经现有 e2e harness（mock 上游）驱动：
  - `call_tool` 成功 → 录得 `meta_tool=call_tool`、`upstream=mock`、`outcome=Ok`、`latency_ms`≥0、`error_kind=None`。
  - 调死/缺上游工具 → `outcome=Error`、`error_kind` 为对应值。
  - `search_tools`/`get_tool_details` → `meta_tool` 正确、`target_tool/upstream=None`。
- fmt/clippy 干净；不回归既有 downstream e2e。

## 4. T3 纲要（T1 后，自己的 spec 细化）

- **`JsonlSink`**：`record()` 把 `serde_json::to_string(rec)` 后 `try_send` 进 `mpsc`；后台 `tokio` writer 任务收行、`append` 写文件、定期/按量 flush。channel 满 → 丢弃 + `warn`（绝不阻塞调用）。
- **`[audit]` 配置**：`enabled: bool`、`path: String`（审计文件）。`mcpgw` 启动期：`enabled` 时建 `JsonlSink`（开 writer 任务）并加入 sinks。
- **T3 自己的 spec 要定的**：文件轮转/retention（按大小/按天？）、写失败/磁盘满的降级、关闭时 flush、并发实例对同一文件的处理。
- **测试**：writer 记 N 条 → 文件 N 行合法 JSON；channel 满丢弃路径；关闭 flush。

## 5. 错误处理与不变量

- sink **同步、非阻塞、绝不 panic、绝不阻塞调用**：`record` 失败（如 channel 满）只记 `warn` 并丢弃该条观测——**观测故障绝不影响工具调用本身**。
- **仅元数据**是硬不变量：`observe` 的公开类型**无法**承载参数/返回内容（只有 `*_bytes`），从类型上杜绝 payload 泄露。
- `metatools` 保持纯逻辑，不依赖 `observe`。

## 6. 实现期需现场确认/可能回退的点

- `GatewayServer::new` 签名新增 sinks：全调用点（downstream stdio/http 装配、`tests/common`、`tests/server.rs`、`tests/http_server.rs`、mcpgw）编译实证。倾向默认注入 `[TracingSink]`，测试用捕获 sink。
- `arg_bytes`/`result_bytes` 的口径：对入参 `Map` 与出参 `CallToolResult` 的 `serde_json` 序列化字节数（一致、可复现）；确认不因此**额外**序列化大对象造成开销（result 已要序列化回客户端，可复用其长度）。
- search/get_details 的 `error_kind`：这两条几乎不出错（get_details 缺工具走 `tool_not_found`）；以实际代码路径为准。
- `ts_unix_ms` 取 `SystemTime::now()`；tracing 行另有自己的时间戳，二者独立。
