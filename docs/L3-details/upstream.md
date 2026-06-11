# L3 — `upstream` 细节

## rmcp 1.7 client 用法

- **建连**：`().serve(transport).await` —— 用单元类型 `()` 作为 client handler（不处理服务器→客户端请求），
  返回 `RunningService<RoleClient, ()>`。`UpstreamHandle` 持有它。
- **列工具**：`client.list_all_tools().await` —— 自动翻页拉全量 `Vec<rmcp::model::Tool>`。
- **调用**：`client.call_tool(CallToolRequestParams::new(name).with_arguments(args)).await`。
- **关闭**：`client.cancel().await` —— 优雅取消运行中的服务。

## transport-generic `connect`（rmcp `IntoTransport`）

`connect`/`connect_with_trigger` 对传输泛型（`T: IntoTransport<RoleClient, E, A>`），所以同一条建连路径同时支持：

- **生产**：真实 stdio 子进程（rmcp `TokioChildProcess`，由 `connect::connect_stdio_upstream` 构造）。
- **测试**：`tokio::io::duplex(4096)` 的内存双工管道——无需起进程即可端到端验证。

（早期裸 `AsyncRead + AsyncWrite + Send + Unpin + 'static` 约束已升级为 rmcp 的 `IntoTransport`，以同时吃
duplex 与 `TokioChildProcess`。）测试用 `MockUpstream::new().serve(server_io)` 起服务端，
`UpstreamHandle::connect(name, client_io)` 起客户端，两端经 duplex 对接。

## list_changed 转发：`UpstreamClientHandler` + `RebuildTrigger`

每条连接都装一个 `UpstreamClientHandler { server, trigger: Option<RebuildTrigger> }`（`RebuildTrigger =
mpsc::Sender<String>`）。rmcp 收到上游的 `tools/list_changed` 通知时回调 `on_tool_list_changed`，handler 把
**该上游名** `try_send` 进 trigger channel：

- 用 `try_send`（非阻塞）：channel 满也不阻塞 rmcp 的通知处理；网关 worker 会 coalesce 同一波突发，丢弃溢出的
  重复触发无害（最终都会触发一次重建）。
- `trigger: None` 时 handler 为 no-op——内存单测（`connect`）不关心 list_changed 时用它。
- 这是 **上游 → 网关** 的单向链路；它**不**外溢到下游客户端（下游元工具集合恒定，见 [downstream L3](./downstream.md)）。

## eager-connect 与降级启动（`connect.rs`）

`connect_all(registry, upstreams, trigger)` 顺序 eager-connect 每个 `UpstreamConfig`，**按 transport 分派**且
**降级启动**：

- `UpstreamTransport::Stdio` → `connect_stdio_upstream`；`UpstreamTransport::Http` → `connect_http_upstream`。
- 任一成功 → `registry.insert` + 记 `connected`；失败 → `warn!` + 记 `skipped`，**不** `Err`。
- 因此一个连不上/起不来的上游（stdio 或 http）只被记录到 `ConnectSummary.skipped`，不阻断网关启动；`serve` 打日志后
  照常做初始 rebuild。

**HTTP 上游复用泛型 `connect_with_trigger`**：M1-B.2 把 `connect`/`connect_with_trigger` 的签名泛化为
`IntoTransport<RoleClient>` 的直接收益——stdio 的 `TokioChildProcess` 与 http 的
`StreamableHttpClientTransport`（reqwest-backed，由 `build_http_client_config` 组装 env-解析后的 bearer/headers）
**共用同一条** list_changed / 握手 / 超时管线，`connect_http_upstream` 无需复制任何握手逻辑。env→client config 的
纯逻辑（缺 env = 硬错误）由单测覆盖，真正的网络建连留待 T6 e2e。

**env allow-list**：`build_command` 先 `c.env_clear()` 清空子进程环境，再仅把 `env_passthrough` 列出、且在
mcpgw 自身环境里存在的变量传入。默认子进程**拿不到**父进程环境，须显式 allow-list（如 `PATH`/`HOME`/凭据变量）。

**握手超时**：`connect_stdio_upstream` 用 `tokio::time::timeout(cfg.call_timeout_ms, connect)` 给
connect/initialize 握手加界——子进程**起得来但从不应答**（hung）时映射为 `UpstreamError::Timeout`，不拖垮降级
启动。这正补上了下文「挂起 peer」一节里 connect 需调用方加超时的缺口。

## `Tool → ToolDef` 字段映射

`tool_to_def` 把 rmcp `Tool` 投影为 `catalog::ToolDef`：

| `Tool` 字段 | 类型 | → `ToolDef` |
|-------------|------|-------------|
| `name` | `Cow<str>` | `name: tool.name.to_string()`（原始名，未命名空间化） |
| `description` | `Option<Cow<str>>` | `tool.description.as_deref().unwrap_or("")` —— **缺省映射为空串** |
| `input_schema` | `Arc<JsonObject>` | `Value::Object((*tool.input_schema).clone())` —— 解引用 `Arc` 并克隆内层 map |

`server` 来自调用方传入；`qualified_name()` 由 `catalog` 派生为 `{server}__{name}`。

## 冲突检测（intra-server，first-dupe-wins）

`ingest_tools` 用 `HashSet<&str>` 记录已见工具名：

- 首次见到某名 → `catalog.upsert(tool_to_def(...))`。
- 再次见到同名 → `tracing::warn!`（含 `server`、`tool`）+ `continue`，**保留首个**。
- 返回值是被跳过的 intra-server 重复数（仅统计本次 `tools` 内的冲突，不与 catalog 既有状态比较；
  重复摄取同 server 会经 `upsert` 覆盖既有条目）。

## 注册表并发模型

`UpstreamRegistry` = `Arc<RwLock<HashMap<String, Arc<UpstreamHandle>>>>`（`Clone + Default`）：

- **锁不跨 await**：`get`/`insert`/`remove` 在锁内只做 map 操作，立刻 `cloned()`/返回后释放锁；任何 `.await`
  （`ingest_into`/`call_tool`）都发生在锁外，避免持锁挂起。
- `get` 返回 `Option<Arc<UpstreamHandle>>`（克隆 `Arc`），`remove` 返回 `Option<Arc<…>>`，调用方可据此 graceful
  `shutdown().await` 或直接丢弃。
- **Arc-drop 即取消**：rmcp 的 `RunningService` 内含 `DropGuard`；当最后一个 `Arc<UpstreamHandle>` 被丢弃
  （如 `insert` 同名覆盖、`remove` 后不持有），底层服务在 drop 时被取消。
- `server_names` 返回**排序**后的名字列表，结果确定、可断言。

## `UpstreamState`：占位枚举

`Connecting` / `Ready` / `Failed` 目前只是定义（仅单测断言三者互不相等），尚未与注册表/连接生命周期联动。
M1-B 网关接入时会用它驱动健康状态与重连策略。

## testkit 与门控集成测试

- `testkit.rs` 用 `#![cfg(any(test, feature = "testkit"))]` 门控。`MockUpstream` 经 rmcp `#[tool_router]` /
  `#[tool]` / `#[tool_handler]` 宏暴露**固定**的 `echo`（回显入参 `text`）、`greet`（返回 `"hello"`）与 `slow`
  （sleep 远超任何合理超时，用于触发 `call_timeout`）。
- `RevealingMockUpstream` 手写 `ServerHandler`（声明 `enable_tool_list_changed`）：`list_tools` 初始返回
  `echo` + `reveal`，`call_tool("reveal")` 把内部 `AtomicBool` 置位、`ctx.peer.notify_tool_list_changed()`
  发通知，此后 `list_tools` 再多返回 `late_tool`。用于端到端驱动 reveal → notify → rebuild → search 闭环。
- testkit-only 二进制 `mock-stdio`（`required-features = ["testkit"]`）把 `MockUpstream` 跑在真实 stdio 上，
  让子进程 connect 路径（`connect_stdio_upstream`）能对接真实子进程冒烟。
- 集成测试 `tests/integration.rs` 依赖这些 mock，故在 `Cargo.toml` 用 `[[test]] required-features = ["testkit"]`
  门控：`cargo test --all-features` 编译并运行它；裸 `cargo test` 则**跳过**该 target（不编译失败）。

## HTTP 上游 e2e（`tests/http_connect.rs`）

- M1-C T6 用 rmcp `StreamableHttpService`（serving `MockUpstream`，`LocalSessionManager` + 默认
  `StreamableHttpServerConfig`）在 axum 上挂到 `/mcp`、bind `127.0.0.1:0` 取临时端口，起一个**真实的 HTTP MCP 上游**，
  再用 `connect_http_upstream` 经真网络往返连接，端到端验收 T2 的连接/鉴权路径（握手、`IntoTransport` 绑定、头透传）。
- axum 外挂一层 `from_fn_with_state` **记录请求头中间件**（`record_headers`）把每个请求的 `Authorization` 头存进
  `Arc<Mutex<Vec<…>>>`；因 initialize / list_tools / call_tool 各发一次 HTTP 请求，会记录多条，故断言用 `.any()`。
- 断言三事：工具被命名空间摄取（`remote__echo`）、`call_tool("echo")` 转发并回显、上游确实收到
  `Authorization: Bearer topsecret`（T2-fix 存**原始** token，rmcp reqwest 客户端在线上加 `Bearer ` 前缀）。
- **注意**：断言用的 `headers.lock()` guard 必须在 `handle.shutdown().await` **之前**释放——shutdown 会向上游发
  session 终止请求、再次穿过 `record_headers` 中间件去 `lock()` 同一 mutex，若 guard 仍被持有则死锁。
- 该 target 同样以 `required-features = ["testkit"]` 门控（`MockUpstream` 在 `testkit` 后面）；dev-deps 额外引入
  `axum` 0.8 与 rmcp 的 `server` / `transport-streamable-http-server` 等 feature 仅供该测试编译。

## 观察到的行为（失败语义）

- **崩溃 / 关闭的 peer**：丢弃 duplex 的服务端会让客户端 `initialize` 因 EOF **快速报错**，`connect` 立即返回
  `Err(UpstreamError::Connect)`。
- **挂起（hung）的 peer**：服务端存活但从不应答，则裸 `connect` 会一直阻塞。生产路径
  `connect::connect_stdio_upstream` 已用 `tokio::time::timeout(call_timeout_ms, …)` 包住握手，把挂起映射为
  `UpstreamError::Timeout`，使单个挂起上游不拖垮其余网关（集成测试 `one_upstream_failure_does_not_block_others`
  即验证此点）。摄取期的挂起则由 `gateway` 的 per-ingest 超时隔离（见 [gateway L3](./gateway.md)）。

> **超时的取消语义（已知限制，留 M1-C）**：`call_tool` / `ingest_into` 超时时仅**丢弃**在途的 rmcp 请求 future，
> **不**向真实上游发协议级 `notifications/cancelled`。Rust 语义下丢弃 future 即释放本地等待资源；上游迟到的响应在
> rmcp 客户端侧因请求 id 无匹配而被丢弃。因此当前实现安全但非「主动取消」——真实子进程上的主动取消通知留待 M1-C。

## 测试覆盖

- 单测（`mapping.rs`）：命名空间+字段拷贝、缺省 description、保留 input_schema、计数 dupes。
- 单测（`registry.rs`）：`UpstreamState` 三值互异。
- 单测（`connect.rs`）：`build_command` env allow-list（仅传 allow-listed 变量、清除其余）。
- 单测（`lib.rs` spike）：client 经 duplex 看到 mock 三工具。
- 集成（`tests/integration.rs`，需 `testkit`）：摄取命名空间工具、转发 `call_tool`、registry 取/删、单上游失败不阻塞其余、
  `call_tool` 慢于 `call_timeout` 时映射为 `UpstreamError::Timeout`、经 `mock-stdio` 子进程的真实 stdio connect 冒烟。
- e2e（`tests/http_connect.rs`，需 `testkit`）：经真实 axum `StreamableHttpService` 上游 + 记录头中间件，验收
  `connect_http_upstream` 的网络往返——摄取、`call_tool` 回显、上游收到 `Authorization: Bearer topsecret`。

## 相关

- 接口见 L2：[upstream](../L2-components/upstream.md)
- 逐文件 API 见 L4：[mapping](../L4-api/upstream-mapping.md) · [connection](../L4-api/upstream-connection.md) ·
  [connect](../L4-api/upstream-connect.md) · [registry](../L4-api/upstream-registry.md)
