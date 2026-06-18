# mcpgw 调用内容捕获与检视（call payload capture）设计

日期：2026-06-18
状态：已批准设计（待写实施计划）

## 背景与目标

试用 M3 下钻后，用户发现 Call detail **看不到调用的参数**，并希望在各处「看到参数」+ 对详情页的 Recent calls 列表**支持筛选（含按参数筛选）**。

根因是**刻意的设计**：调用观测 `observe::CallRecord` 是**仅元数据**的（只存 `arg_bytes`/`result_bytes` 字节数，绝不存参数/结果内容），喂给 `TracingSink`/`JsonlSink`（审计）/`MetricsSink`，并有锁死的「9 键仅元数据」测试与「query/参数/结果内容绝不进调用 sink」的隐私文档。要看参数必须**捕获内容**，这反转了「默认仅元数据」原则。

**用户已知情并明确选择「始终捕获」**：每次调用无条件捕获参数 + 结果 + 错误文本，面板总能看。本设计在**不破坏审计/日志隐私洁净**的前提下实现它——内容只活在**面板内存**里，元数据路径（tracing/audit JSONL/metrics）保持不变。

### 目标
1. 每次 meta-tool 调用捕获 **arguments + 结果内容 + 上游错误文本**，存进面板的逐条内存环（单条截断防爆）。
2. Call detail（`/api/calls/{id}`）展示 arguments / result / error；列表（`/api/calls`）保持轻（不含内容）。
3. Recent-calls 列表与主 Calls 页支持筛选：标准维度（outcome/meta/上游/工具/时间，已有）+ **内容自由文本搜索** + **结构化 `key=value` 参数过滤**（内容类过滤只对 `source=live`）。

### 非目标
- 不把内容写进 audit JSONL / tracing（审计/日志**保持仅元数据**，锁死测试不破）。
- 不做内容脱敏/加密/访问控制（用户选择「始终捕获」，明确接受隐私权衡）。
- 不持久化内容（重启即丢；内容只在 `call_buffer` 上界的内存环里）。
- 不对 history（audit 回放）做内容展示或内容过滤（audit 无内容）。

## 架构（方案 A：独立富内容通道，元数据路径不动）

埋点处 `downstream::GatewayServer::call_tool` 在现有「仅元数据」`CallRecord` 扇出**之外**，额外构造一条**富内容**记录并扇出到一条**新 sink 通道**——只在面板启用（存在内容 sink）时执行。面板的逐条环改为消费这条富通道，于是每条 call 自带内容；元数据 `CallRecord` 继续喂 tracing/audit/metrics，**零改动**。

```
downstream::call_tool（一次调用）
├─ CallRecord（仅元数据，不变）──fan-out──▶ TracingSink / JsonlSink(audit) / MetricsSink
└─ CallContent（args + result，截断）──fan-out──▶ CallContentSink（dashboard CallRingSink）
                                                    └─ 环存「元数据+内容」，分配 seq id
```

### 组件 1：`observe`（新增契约，镜像 `DiscoverySink`）
```rust
/// 一次调用的内容载荷（与仅元数据的 `CallRecord` 物理隔离）。字段是已序列化、已截断的 JSON 文本，
/// 便于存储/子串搜索/`<pre>` 渲染；超长则截断并置 *_truncated。
pub struct CallContent {
    pub args: String,
    pub args_truncated: bool,
    pub result: String,
    pub result_truncated: bool,
}

/// 内容扇出目标：同时拿到元数据 `CallRecord` 与内容 `CallContent`（避免重复元数据字段）。
/// 由 dashboard 的逐条环实现。实现必须非阻塞、不 panic（与 `CallSink`/`DiscoverySink` 同契约）。
pub trait CallContentSink: Send + Sync {
    fn record(&self, meta: &CallRecord, content: &CallContent);
}
```
放 `crates/observe/src/`（与 `CallRecord`/`DiscoveryRecord` 并列）。`observe` **不引入新依赖、不碰 `CallRecord`**。

### 组件 2：`downstream`（埋点捕获 + 截断）
- `GatewayServer` 新增 `content_sinks: Arc<[Arc<dyn observe::CallContentSink>]>` 与 `payload_max_bytes: usize`（构造期注入，镜像现有 `discovery` sinks / `top_k`）。
- `call_tool` 尾部，在 `for sink in self.sinks.iter() { sink.record(&rec); }` 之后追加：
  ```rust
  if !self.content_sinks.is_empty() {
      let (args_s, args_trunc) = cap_json(&args, self.payload_max_bytes);
      let (result_s, result_trunc) = cap_response(&response, self.payload_max_bytes);
      let content = observe::CallContent {
          args: args_s, args_truncated: args_trunc,
          result: result_s, result_truncated: result_trunc,
      };
      for s in self.content_sinks.iter() { s.record(&rec, &content); }
  }
  ```
  - `cap_json(value, cap) -> (String, bool)`：紧凑 JSON 序列化 + UTF-8 安全截断到 `cap` 字节，返回 `(截断后字符串, 是否截断)`；序列化失败返回 `("<unserializable>", false)`。
  - `cap_response(response, cap) -> (String, bool)`：序列化 `response`（`Ok(CallToolResult)` → 其 JSON；失败时 `CallToolResult::error` 的内容即上游错误文本，故 result 同时承载错误文本）后同样截断。
- `args` 是 meta-tool 调用的完整入参（call_tool 的真实上游参数嵌套在 `args["arguments"]`，原样保留，便于 `key=value` 命中 `text` 等嵌套键）。

### 组件 3：`dashboard` 数据层
- `CallRingSink`：从 `impl CallSink`（仅元数据）改为 `impl observe::CallContentSink`（元数据+内容）。`StoredCall` 现存 `{ seq, record: CallRecord, content: CallContent }`。
- `CallItem` 增可选内容字段：`args: Option<String>`、`args_truncated: bool`、`result: Option<String>`、`result_truncated: bool`。
  - `query()`（列表）投影**不含内容**（`args/result = None`）——列表轻。
  - `get(seq)`（详情）投影**含内容**。
- `CallFilter` 增 `q: Option<String>`、`arg_key: Option<String>`、`arg_val: Option<String>`。`matches()` 在 `StoredCall` 的**完整内容**上判定：
  - `q`：对 `args + result` 文本做**大小写不敏感子串**匹配。
  - `arg_key`/`arg_val`：把 `args` 文本 `serde_json::from_str` 回 `Value`，**递归找键 `arg_key`** 且其值字符串化后**含 `arg_val`**（截断导致解析失败时该过滤不命中——best-effort，args 极少触达 16KB 截断）。
- `main.rs` 接线：面板启用时，把 `CallRingSink` 作为 **`CallContentSink`** 注入 `content_sinks`（stdio + http 两个 `GatewayServer`），**不再**进元数据 `sinks`；`MetricsSink` 仍在元数据 `sinks`。
- 内容过滤只对 live：`source=history` 时 `replay_audit_calls` 无内容，`q`/`arg_*` **被忽略**（不应用）。

### 组件 4：`config`
- `[dashboard]` 新增 `payload_max_bytes: usize`，默认 `16384`（16KB）；`validate` 在 `dashboard.enabled` 时**拒绝 `= 0`**（与现有 `trace_buffer`/`call_buffer` 校验一致）。注入 `GatewayServer`。
- **始终捕获**：无独立开关——面板启用即捕获（用户选择）。面板关闭则无 `content_sinks`、零开销。

### 组件 5：前端
- **CallDetail（`#/calls/{id}`）**：新增 **Arguments / Result** 两块（失败时 result 即含上游错误文本），`<pre>` 渲染（先 `JSON.parse`+`stringify(,2)` 美化，失败则原样；文本插值、**无 `{@html}`**）。截断显示「(truncated)」；live 环淘汰后无内容显示「(content not retained)」。
- **过滤 UI**：
  - 主 **Calls 页**：现有 source/meta/outcome chips + 分页之上，加**内容搜索框**（`q`）+ **arg 过滤**（`key` 与 `value` 两个输入 → `arg_key`/`arg_val`）。
  - **UpstreamDetail / ToolDetail 的 Recent calls**：加 **outcome chips + 内容搜索框**（已按 upstream/tool 固定、source=live）。
  - `source=history` 时内容过滤输入禁用（灰显）。
- **「其他地方看参数」**：每个 Recent-calls 行可点进 CallDetail（现含参数）——三处详情页都能下钻看参数。

## 数据流

```
调用 ──▶ call_tool
  ├─ CallRecord ──▶ Tracing/Audit/Metrics（仅元数据，不变）
  └─ CallContent ──▶ CallRingSink 环（元数据+内容，seq id）
                       ├─ GET /api/calls?...&q=&arg_key=&arg_val=  ──▶ 列表（内容轻）+ 内容过滤(live)
                       └─ GET /api/calls/{id}                      ──▶ 详情（含 args/result）
                            └─ CallDetail 渲染 Arguments/Result/Error
```

## 错误处理
- 序列化失败：`cap_json`/`cap_response` 失败时存空串或 `"<unserializable>"`，不 panic（`CallContentSink::record` 契约非阻塞不 panic）。
- 截断：UTF-8 安全（按字符边界截）；置 `*_truncated`。
- history + 内容过滤：忽略内容过滤（无内容可匹配），不报错。
- live 环淘汰：详情 `args/result = None`，前端显示「content not retained」。

## 测试策略
- observe：`CallContent`/`CallContentSink` 形状（若加单测）。
- downstream：埋点同时扇出元数据与内容；截断生效；元数据 `CallRecord` 仍仅元数据（既有锁死测试不变、仍绿）；`content_sinks` 为空时零开销。
- dashboard：`CallRingSink` 实现 `CallContentSink`、`StoredCall` 存内容、`get` 含内容、`query` 不含内容；`CallFilter` 的 `q`（子串）与 `arg_key/arg_val`（递归键值、截断 best-effort）；history 忽略内容过滤。
- config：`payload_max_bytes` 默认 16384、`=0` 校验。
- e2e（mock 上游）：`call_tool mock__echo` 后 `/api/calls/{id}` 含 `args`（含 `text`）与 `result`；`?q=hi` 命中、`?arg_key=text&arg_val=hi` 命中；列表不含内容。
- 门禁：`cargo fmt --all --check`、`clippy --all-targets --all-features -D warnings`、`cargo test --all-features`、`build --locked`；前端 `npm run build` + dist 入库 + `{@html}` 守护测试。

## 里程碑
- **M1 捕获 + 详情展示**：observe `CallContent`/`CallContentSink` + downstream 捕获/截断 + `GatewayServer` 接线 + dashboard `CallRingSink`→`CallContentSink` + `CallItem` 内容 + `/api/calls/{id}` 返回内容 + `[dashboard].payload_max_bytes` + CallDetail 展示 Arguments/Result。**直接回答主诉求。**
- **M2 过滤**：`/api/calls` 的 `q` + `arg_key/arg_val`（live-only）+ 主 Calls 页与详情页 Recent-calls 的过滤 UI（含内容搜索/arg 过滤、history 禁用内容过滤）。

## 隐私说明（务必在文档与代码注释中点明）
- 内容**只在面板内存**（`call_buffer` 上界、`payload_max_bytes` 单条上界），**重启即丢**，**绝不**写入 audit JSONL / tracing / metrics——审计落盘与日志**保持仅元数据**，锁死测试不破。
- 这是用户**知情选择**的「始终捕获」：面板内存里会留可能含密钥/PII 的调用内容；面板**仅 localhost、默认关闭**，与现有面板同信任模型。
