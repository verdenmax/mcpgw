# L3 — `upstream` 细节

## rmcp 1.7 client 用法

- **建连**：`().serve(transport).await` —— 用单元类型 `()` 作为 client handler（不处理服务器→客户端请求），
  返回 `RunningService<RoleClient, ()>`。`UpstreamHandle` 持有它。
- **列工具**：`client.list_all_tools().await` —— 自动翻页拉全量 `Vec<rmcp::model::Tool>`。
- **调用**：`client.call_tool(CallToolRequestParams::new(name).with_arguments(args)).await`。
- **关闭**：`client.cancel().await` —— 优雅取消运行中的服务。

## transport-generic `connect<T: AsyncRead + AsyncWrite>`

`connect` 对传输泛型（`T: AsyncRead + AsyncWrite + Send + Unpin + 'static`），所以同一条建连路径同时支持：

- **生产**：真实 stdio 子进程（rmcp `transport-child-process` / `transport-io`）。
- **测试**：`tokio::io::duplex(4096)` 的内存双工管道——无需起进程即可端到端验证。

测试用 `MockUpstream::new().serve(server_io)` 起服务端，`UpstreamHandle::connect(name, client_io)` 起客户端，
两端经 duplex 对接。

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

- `testkit.rs` 用 `#![cfg(any(test, feature = "testkit"))]` 门控；`MockUpstream` 经 rmcp `#[tool_router]` /
  `#[tool]` / `#[tool_handler]` 宏暴露 `echo`（回显入参 `text`）、`greet`（返回 `"hello"`）与 `slow`
  （sleep 远超任何合理超时，用于触发 `call_timeout`）。
- 集成测试 `tests/integration.rs` 依赖该 mock，故在 `Cargo.toml` 用 `[[test]] required-features = ["testkit"]`
  门控：`cargo test --all-features` 编译并运行它；裸 `cargo test` 则**跳过**该 target（不编译失败）。

## 观察到的行为（失败语义）

- **崩溃 / 关闭的 peer**：丢弃 duplex 的服务端会让客户端 `initialize` 因 EOF **快速报错**，`connect` 立即返回
  `Err(UpstreamError::Connect)`。
- **挂起（hung）的 peer**：服务端存活但从不应答，则 `connect` 会一直阻塞——必须由调用方加 `tokio::time::timeout`
  才能让单个挂起上游不拖垮其余网关（集成测试 `one_upstream_failure_does_not_block_others` 即验证此点）。

## 测试覆盖

- 单测（`mapping.rs`）：命名空间+字段拷贝、缺省 description、保留 input_schema、计数 dupes。
- 单测（`registry.rs`）：`UpstreamState` 三值互异。
- 单测（`lib.rs` spike）：client 经 duplex 看到 mock 三工具。
- 集成（`tests/integration.rs`，需 `testkit`）：摄取命名空间工具、转发 `call_tool`、registry 取/删、单上游失败不阻塞其余、
  `call_tool` 慢于 `call_timeout` 时映射为 `UpstreamError::Timeout`。

## 相关

- 接口见 L2：[upstream](../L2-components/upstream.md)
- 逐文件 API 见 L4：[mapping](../L4-api/upstream-mapping.md) · [connection](../L4-api/upstream-connection.md) ·
  [registry](../L4-api/upstream-registry.md)
