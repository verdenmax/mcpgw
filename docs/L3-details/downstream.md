# L3 — `downstream` 细节

## HTTP 鉴权层细节（`http.rs` 的 `require_api_key` 中间件）

`build_router` 在 `api_keys` 非空时叠加一层 axum `from_fn_with_state` 中间件 `require_api_key`：

- **Bearer 提取**：`presented_bearer` 从 `Authorization` 头读字符串，用 `split_once(' ')` 拆成 `scheme` 与 `token`；
  **scheme 经 `scheme.eq_ignore_ascii_case("bearer")` 大小写不敏感匹配**（故 `Bearer`/`bearer`/`BEARER` 均接受），
  **token 值仍大小写敏感**地原样取出。缺头/非 ASCII/无空格分隔/scheme 非 `bearer` 都视为「未呈现」。**`Bearer ` 后为空串的
  token 也经 `token.is_empty()` 判定视为「未呈现」**（audit F1，故 `Authorization: Bearer ` 这类空令牌一律 → 401，
  而非以空串去做密钥比较）。
- **常量时间比较**：`key_authorized` 用 `subtle::ConstantTimeEq::ct_eq` 对每个配置 key 与呈现值逐字节比较，
  把结果按位 `|=` 累积成 `matched`（**不**在命中后提前 `return`，避免泄露「命中了第几个 key」的时序）。
  `ct_eq` 对长度不同的 `&[u8]` 会短路返回 `Choice(0)`——只泄露长度，可接受；长度相同时做常量时间比较。
- **401 不回显期望值**：校验失败一律 `StatusCode::UNAUTHORIZED.into_response()`，**不**在响应里带任何关于
  期望 key 的信息（不区分「缺 key」「错 key」的错误体）。
- **keyset 为空时放行**：不挂这层中间件，依赖 **localhost 绑定 + rmcp `allowed_hosts`**
  （默认 `[localhost, 127.0.0.1, ::1]`）作为唯一防线。

## rmcp `ServerHandler` 三个方法

`GatewayServer` 手写实现 `ServerHandler`（不用 `#[tool_router]` 宏，因为元工具是固定三件套、且 `call_tool`
要自定义分派与错误映射）：

- **`get_info`**：`ServerInfo::new(ServerCapabilities::builder().enable_tools().build())` + 构建环境信息。
  **只** `enable_tools`，**不** `enable_tool_list_changed`——见下文。
- **`list_tools`**：直接 `ListToolsResult::with_all_items(meta_tools())`，忽略分页参数，恒返回 3 个元工具。
- **`call_tool`**：按 `request.name` 分派（下表）。

## `call_tool` 分派表

| 客户端调用名 | 取参 | 委派 | 成功返回 | 失败/缺省 |
|--------------|------|------|----------|-----------|
| `search_tools` | `query: &str`（缺省 `""`）、`top_k: u64`（缺省 `self.default_top_k`） | `metatools::search_tools(&snap, query, top_k)` | 命中数组的 JSON 文本 | 序列化失败 → `McpError::internal_error` |
| `get_tool_details` | `name: &str`（缺省 `""`） | `metatools::get_tool_details(&snap, name)` | 找到 → `ToolDef` JSON 文本 | `None` → `isError`（`"no such tool: {name}"`） |
| `call_tool` | `name: &str`（缺）、`arguments: object`（可选） | `metatools::call_tool(&snap, registry, name, inner).await` | 上游的 `CallToolResult` 透传 | 缺 `name` → `isError`；`MetaError` → `isError` |
| 其它名 | — | — | — | `McpError::invalid_params("unknown tool: {other}")` |

每次分派都先 `let snap = self.state.snapshot()`（**无锁**加载当前快照），`call_tool` 另取
`self.state.registry()` 做上游转发。

## `MetaError` → `isError`，未知名 → `McpError`

两类失败被刻意区别对待：

- **业务/运行期失败**（工具不存在、上游不可用、上游超时、上游报错）来自 `metatools`，被包成
  `CallToolResult::error(...)`（即 MCP 的 `isError: true`）。客户端拿到的是一次**成功的 MCP 调用**、内容标记为
  错误——符合 MCP「工具执行错误经结果体回传」的约定，LLM 可读取并重试。
- **协议级误用**（调用了一个根本不存在的元工具名）返回 `Err(McpError::invalid_params)`，即 JSON-RPC 错误。
  这区分了「这个工具运行失败」与「你叫错了工具」。

## 调用观测（M6.T1，仅元数据）

`call_tool` 在分派之外还**在调用边界构造一条 `observe::CallRecord` 并扇出给 `self.sinks`**——这是 T1
（tracing）与 T3（审计 JSONL）共享的「instrument → multi-sink」埋点。**埋点只在 `downstream`**：
`metatools` crate 保持纯函数、**不**依赖 `observe`。

- **埋点位置**：埋点逻辑全在 `GatewayServer::call_tool` 内。分派 `match` 的每个臂返回五元组
  `(response, meta_tool, target_tool, outcome, error_kind)`；`match` 之后统一构造记录、`for sink in
  self.sinks.iter() { sink.record(&rec); }` 同步扇出，最后返回 `response`。空 `sinks` 即「不观测」。
- **延迟测量基准**：进入即 `let started = Instant::now()`；`match` 一结束**立刻** `latency_ms =
  started.elapsed()`，**早于**结果再序列化（`result_bytes`）与 `upstream` 派生。故 `latency_ms` 反映
  **分派本身**，不含观测记账开销。
- **`arg_bytes` / `result_bytes` 基准**：经私有 `json_len(value)` 量取——一个仅计数的 `CountingWriter` 配合
  `serde_json::to_writer`，量取序列化 JSON 字节长度而**不分配中间 `String`**（数值与旧的 `to_string().len()` 一致）。
  `arg_bytes = json_len(&args)`（进入时算一次）；`result_bytes = json_len(&response)`，`Err`（协议错误）路径记 `0`。两者都是
  **字节数（size）**，**绝不含**任何参数/结果内容。
- **`upstream` 派生（安全修复）**：对 `target_tool` 做**工具目录解析**取真实 server——
  `get_tool_details(&snapshot, target_tool).map(|def| def.server.clone())`（如 `github__create_issue` 解析到
  其 `def.server = github`）；只有 `call_tool` 带能解析的 `target_tool` 时才有值，**解析不到则 `None`**。
  **不再** `split_once("__")` 切 client 提供的名字：否则一个未知/构造的 `call_tool` 名（`ToolNotFound`）会切出
  一个**无界、attacker 可控**的 `upstream` 前缀，既污染指标又能灌爆 dashboard `per_upstream` 维度。
- **`target_tool` 限长（边界修复）**：记录的 `target_tool` 是 **client 提供的名字**（`ToolNotFound` 路径上
  完全 attacker 可控）。派生完 `upstream`（用完整名解析，保证合法长名仍能命中目录）后，经私有
  `clamp_tool_name(&t)` 按 **char** 截断到 `MAX_TARGET_TOOL_CHARS = 256`（UTF-8 安全、不切码点）再入记录。
  否则**计数有界**的调用环（`call_buffer` 条）每条体积无界，且审计 JSONL 每行随 client 输入膨胀——与 `clamp_query`
  对发现 query 的限长同理（按 client 输入限长，而非仅限条数）。
- **`outcome` / `error_kind` 分类**：`call_tool` 转发失败经私有 `classify(&MetaError)` 映射，其余由分派臂
  内联给出（完整规范表以 L4 [downstream-lib](../L4-api/downstream-lib.md) 的「`error_kind` 取值表」为准）：

  | 触发情形 | `outcome` | `error_kind` |
  |----------|-----------|--------------|
  | 任一元工具序列化结果失败（`McpError::internal_error`） | `error` | `internal` |
  | `get_tool_details` 找不到工具（`None`） | `error` | `tool_not_found` |
  | `call_tool` 缺 `name` | `error` | `invalid_params` |
  | `MetaError::Timeout` | `timeout` | `timeout` |
  | `MetaError::Call` | `error` | `upstream_call` |
  | `MetaError::ToolNotFound` | `error` | `tool_not_found` |
  | `MetaError::UpstreamUnavailable` | `error` | `upstream_unavailable` |
  | `call_tool` 成功往返但上游结果 `is_error=true`（结果原样转发，仅观测判 `error`） | `error` | `upstream_tool_error` |
  | 成功 | `ok` | `None` |

- **仅元数据不变量**：记录的类型本身就装不下载荷——只有上述 size 与分类字段，故观测**绝不泄露
  secret/PII**。`observe` 的单测把序列化 key 集合锁死为恰好这 9 个键。
- **未知元工具名不记录**：调用了一个根本不存在的元工具名时，`call_tool` **早退** `Err(McpError::invalid_params)`
  ——这是**协议误用**、不是一次网关工具调用，因此**不**构造记录、**不**扇出。
- **默认 sink**：`mcpgw serve` 注入 `[observe::TracingSink]`，把每条记录发为结构化
  `tracing::info!(meta_tool, target_tool, upstream, latency_ms, outcome, error_kind, arg_bytes,
  result_bytes, "tool_call")` 事件（走 stderr，与日志同流）。stdio 与 HTTP 两条传输**共享同一切片**。

## 发现追踪捕获（dashboard，opt-in，与仅元数据隔离）

除上述仅元数据扇出外，`search_tools` 分支在 `self.discovery` **非空**时再扇出一条
`observe::DiscoveryRecord`——这是 dashboard 子系统 A 的搜索发现追踪，**与 `CallSink` 通道物理隔离**：

- **何时捕获**：私有 `discovery_record_for_search(query, top_k, &hits, started.elapsed())` 把 query、`top_k`、
  命中工具映为 `Vec<DiscoveryHit { name, score }>`（即 `ToolSummary.score` 的去处）、latency 构成
  `DiscoveryRecord`，`for sink in self.discovery.iter() { sink.record(&drec) }` 扇出。空 catalog → 空
  `results`，仍追踪。
- **隔离与隐私**：`DiscoveryRecord` 含 **query 文本 + 工具名**，**绝不**进 `self.sinks`（tracing/审计仅元数据
  通道），只进独立的 `DiscoverySink`。装配仅在 `[dashboard].trace_queries = true` 时注入该 sink，故**默认空
  切片、不捕获**。
- **非阻塞**：dashboard 的 `DiscoveryRingSink` 写内存 ring + `try_send` 可选 JSONL（满则丢弃），故扇出不阻塞
  `search_tools` 热路径。详见 [dashboard L3](./dashboard.md)。

## 调用内容捕获（M1，dashboard，与仅元数据/审计隔离）

在仅元数据 `CallRecord` 扇出**之外**，`call_tool` 在 `self.content_sinks` **非空**时再扇出一条
`observe::CallContent`——这是 dashboard 子系统 A 的逐条调用参数/结果文本通道，**与 `CallSink`（tracing/审计）
物理隔离**：

- **何时捕获**：元数据 `for sink in self.sinks` 循环**之后**、`if !self.content_sinks.is_empty()` 才付构造成本：
  `cap_json(&args, payload_max_bytes)` 序列化并截断参数、`cap_response(&response, payload_max_bytes)` 截断结果
  （`Err` 协议错误路径截断其错误字符串），组成 `CallContent { args, args_truncated, result, result_truncated }`，
  `for s in self.content_sinks.iter() { s.record(&rec, &content) }` 扇出。内容块是**纯增量**、在元数据循环之后，
  故对元数据通道**零影响**。
- **UTF-8 安全截断**：私有 `truncate_utf8(s, cap)` 按字节封顶但向下退到 `is_char_boundary`，**绝不切码点**，返回
  `(截断后串, 是否截断)`；`cap_json` = compact 序列化 + `truncate_utf8`（序列化失败 → `"<unserializable>"`），
  `cap_response` 对 `Ok` 走 `cap_json`、对 `Err` 截断 `e.to_string()`。封顶值来自 `[dashboard].payload_max_bytes`
  （默认 16384）。
- **隔离与隐私**：`CallContent` **故意不实现 `Serialize`**——类型层面就进不了 JSONL 审计/tracing；它**绝不**进
  `self.sinks`，只进独立的 `CallContentSink`。装配时 dashboard 的 `CallRingSink` 是**唯一**的 `CallContentSink`，
  放进 `content_sinks`（绝不在元数据 `sink_vec` 里）。故内容**只活在内存调用环**，供 dashboard 详情页**实时**展示/
  过滤，**绝不落盘、绝不进审计**。history 回放的调用项内容为 `None`。
- **驻留内存上界**：内容常驻 ≈ `call_buffer × 2 × payload_max_bytes`（args 与 result 各自封顶）。
- **非阻塞**：`CallRingSink` 写内存 ring + `try_send` 可选 JSONL，不阻塞热路径。内容过滤（`q`/`arg_key`+`arg_val`，
  仅 live）与剥离细节见 [dashboard L3](./dashboard.md)。

## 为何 `get_info` 只 `enable_tools`、不 `enable_tool_list_changed`

下游对外暴露的工具列表是**静态的三件套**，永远不变。即便上游 `tools/list_changed` 触发网关重建快照，改变的
只是**被检索/可路由的上游工具**，而非客户端能看到的元工具集合。因此下游**不**声明 `list_changed` 能力、也从不
向客户端发该通知。`list_changed` 是 **上游 → 网关** 的单向关注点（由 `upstream::UpstreamClientHandler` +
`gateway::run_rebuild_worker` 处理），不外溢到 **网关 → 客户端**。

## 端到端测试线束（`tests/`）

`tests/server.rs` + `tests/common/mod.rs` 用 `tokio::io::duplex` 起一对内存管道：服务端跑 `GatewayServer`、
客户端是裸 rmcp `()` client，免起进程即可端到端验证：

- `list_tools_returns_exactly_the_three_metatools`：锁定 3 件套不变量。
- `call_tool_dispatches_all_three_metatools`：挂 `MockUpstream`，验证三条分派路径（search 命中 `mock__echo`、
  details 取定义、call 转发回上游）。
- `call_tool_unknown_meta_name_is_protocol_error`：未知名 → 协议错误。
- `call_tool_routes_missing_upstream_tool_to_iserror`：`MetaError::ToolNotFound` → `isError`。
- `list_changed_refreshes_what_search_can_find`：挂 `RevealingMockUpstream` + 真实 `run_rebuild_worker`，经网关
  调用 `reveal` → 上游发 `tools/list_changed` → handler → trigger → worker 重建 → 轮询直到 `search_tools`
  能搜到新冒出的 `mock__late_tool`。这条覆盖了 reveal → notify → rebuild → search 的完整闭环。
- `meta_tool_calls_are_observed_with_metadata`：注入 `observe::CaptureSink`（经
  `connect_to_gateway_with_sinks`），跑 `search_tools` + `call_tool`（命中 `mock__echo`）+ `call_tool`
  （`mock__nope` → `tool_not_found`）+ 未知名，断言**恰好 3 条**记录（未知名不记录）、`target_tool`/`upstream`
  正确派生、成功/失败的 `outcome`/`error_kind` 正确、`arg_bytes`/`result_bytes` 为正。

> 测试线束的默认 sink 为空（`common::no_sinks()`），仅观测专项用例显式注入 `CaptureSink`。

## 相关

- 接口见 L2：[downstream](../L2-components/downstream.md) · [observe](../L2-components/observe.md)
- 逐文件 API 见 L4：[lib](../L4-api/downstream-lib.md) · [observe-lib](../L4-api/observe-lib.md)
- 元工具与错误见：[metatools L3](./metatools.md) · 重建/触发见：[gateway L3](./gateway.md) ·
  上游 list_changed 见：[upstream L3](./upstream.md)
