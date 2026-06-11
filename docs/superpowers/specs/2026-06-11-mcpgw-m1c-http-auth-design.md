# M1-C 设计：Streamable HTTP 传输 + 静态 API-Key 鉴权

> 状态：已通过 brainstorm 评审，待 writing-plans 细化为实现计划。
> 前置：M0 / M1-A / M1-B.1 / M1-B.2 均已合并到 master（HEAD `77e1ed8`）。
> 关联里程碑：roadmap `M1` 的最后一块（M1-C）。

## 1. 目标与范围

M1-C 为网关补齐 HTTP 双向能力与静态鉴权，使其可被远程客户端/隧道访问，并能聚合远程 HTTP 上游。

**范围内：**
- **下游 HTTP**：用 Streamable HTTP 暴露网关的 3 个元工具；多 API-Key Bearer 鉴权。
- **上游 HTTP**：连接远程 HTTP MCP server 作为上游；静态请求头（Bearer / 自定义头）从环境变量读取。
- **进程模型**：`mcpgw serve` 并发跑 stdio + HTTP 两个 server 任务（按配置开关），共享同一 `Arc<GatewayState>`。

**明确不含（继续延后）：**
- 完整 OAuth / DCR / 反向代理正确性 → **M3**。
- 运行时热吐销 / 增删 API-Key（需控制面板）→ **M4**。
- 超时主动向上游发 `notifications/cancelled`（`m1b2-cancel-note`）→ 继续延后；与 HTTP/鉴权正交，Rust 里 drop in-flight future 已是安全的（陈旧响应被 rmcp 客户端按未匹配 id 丢弃）。

## 2. 配置 Schema 扩展

```toml
[server]
stdio = true                          # 默认 true：对下游暴露 stdio
[server.http]
enabled = false                       # 默认 false；true 才启动 HTTP server
bind = "127.0.0.1:8970"               # 默认 localhost；公网暴露请配合隧道/反代(M3)
path = "/mcp"                          # MCP 路由挂载路径，默认 /mcp
[[server.http.api_key]]               # 0..N 个；为空 = 不鉴权（仅靠 localhost 绑定）
name = "claude-desktop"               # 仅用于日志/可观测标识，绝不打印 key 值
env  = "MCPGW_KEY_CLAUDE"             # key 明文只经 env 引用，配置里只存 env 变量名

[retrieval]                           # M0 已有
strategy = "bm25"
top_k = 8

[[upstream]]                          # stdio 上游（M1-B.2 已有）
name = "github"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_passthrough = ["GITHUB_TOKEN"]

[[upstream]]                          # HTTP 上游（M1-C 新增）
name = "remote-search"
transport = "http"                     # UpstreamTransport 新增变体
url = "https://example.com/mcp"
bearer_env = "SEARCH_BEARER"           # 可选 → Authorization: Bearer <env 值>
call_timeout_ms = 30000                # 可选，默认 30s（沿用现有）
[[upstream.header]]                    # 可选自定义头，0..N 个
name = "X-Api-Version"
env  = "SEARCH_API_VER"
```

**配置规则：**
- 启动时把每个 `env` 解析为实际密钥/头值；**任一被引用的 env 缺失 → fail-fast**，错误指明字段名与 env 变量名（不泄露值）。
- `[server.http]` 的 `HttpConfig` 与 `ApiKeyConfig` 各自加 `#[serde(deny_unknown_fields)]`。
- `UpstreamConfig` 因 `#[serde(flatten)]` 内部标签枚举，**不能**加 `deny_unknown_fields`（已有约定，不变）。
- `UpstreamTransport` 现有 `Stdio` 变体不动，新增 `Http { url, bearer_env, headers }` 变体。`header` 数组进 `Http` 变体内。
- **命名空间校验不变**：`upstream.name` 仍禁止含 `__`。
- **密钥原则**：所有 key / token / header 值仅经 env 引用，绝不写进配置或日志。

## 3. 下游 HTTP（方案 A：rmcp StreamableHttpService + tower 鉴权层）

- 复用 `GatewayServer`（不改其 `ServerHandler` 实现）。装配：
  ```text
  let service = StreamableHttpService::new(
      move || Ok(GatewayServer::new(state.clone(), top_k)),   // service_factory：每会话克隆 Arc<GatewayState>
      Arc::new(LocalSessionManager::default()),                // 内存会话管理
      StreamableHttpServerConfig::default(),                   // allowed_hosts 默认 localhost/127.0.0.1/::1
  );
  let app = Router::new().nest_service(path, service).layer(auth_layer);
  axum::serve(TcpListener::bind(bind).await?, app)...
  ```
- **鉴权层**（tower / axum middleware，挂在 MCP 路由外层）：
  - 配置了 ≥1 个 key → 校验请求头 `Authorization: Bearer <k>`，`<k>` 必须 ∈ keyset，否则 **401**（不回显期望值）。
  - **常量时间比较**：逐 key 做定长比较，避免计时侧信道。
  - 0 个 key → 放行（依赖 `bind` 的 localhost 默认 + rmcp `allowed_hosts` 作为防线）。
- `StreamableHttpServerConfig` 默认 `stateful_mode=true`、SSE keep-alive 15s，本里程碑沿用默认，不引入持久 `session_store`（内存会话足够）。

## 4. 上游 HTTP

- 新增 `connect_http_upstream(name, url, bearer, headers, call_timeout, trigger)`：
  - 构造 reqwest client；组装 `StreamableHttpClientTransportConfig { uri: url, auth_header, custom_headers, ..Default }`。
  - **`bearer_env` → `auth_header` 映射**：`bearer_env` 指向的 env 持有**原始 token**，启动时构造 `auth_header = Some(format!("Bearer {token}"))`（即 rmcp `auth_header` 字段为完整 `Authorization` 头值）。未配 `bearer_env` → `auth_header = None`。
  - `[[upstream.header]]` 列表 → `custom_headers: HashMap<HeaderName, HeaderValue>`，每项 `name` 为头名、`env` 指向的值为头值。
  - `StreamableHttpClientTransport::new(client, cfg)` 作为 transport，**复用现有泛型 `connect_with_trigger`**（同一 handshake 超时 + per-call 超时 + `list_changed` 管线）。这正是 M1-B.2 把签名泛化成 `IntoTransport<RoleClient, E, A>` 的收益。
- `connect_all` 按 `transport` 分派 stdio / http；HTTP 上游连接失败同样**降级隔离**（计入 `ConnectSummary.skipped`，不 abort 整体启动）。

## 5. 进程模型与关闭

- `run_serve`：`prepare_state` 之后，按配置 spawn server 任务，共享 `Arc<GatewayState>` 与 list_changed rebuild worker：
  - `[server].stdio` → stdio server（现有路径）。
  - `[server.http].enabled` → HTTP server（§3）。
  - 至少需启用一个；都没启用 → fail-fast。
- **统一关闭**：`tokio::select!` over `{ stdio service.waiting(), HTTP server future, tokio::signal::ctrl_c() }`：任一完成 → 取消 HTTP 的 `cancellation_token`、停 stdio → 走现有 best-effort 上游 `shutdown()`。
- **语义**：含 stdio 时，stdio 客户端断开（stdin EOF）即结束 serve；仅 HTTP（守护进程）时靠 Ctrl-C / SIGTERM 退出。
- **日志仍走 stderr**（stdout 在 stdio 模式下承载 JSON-RPC 流），沿用现有 `tracing_subscriber`。

## 6. 错误处理（分层、隔离、fail-fast）

| 场景 | 处理 |
|------|------|
| 被引用的 key/header env 缺失 | 启动 fail-fast，指明字段名 + env 变量名（不泄露值） |
| 下游请求无 / 错 Bearer | 401，不回显期望值 |
| `bind` 地址占用 / 非法 | 启动 fail-fast，明确报错 |
| HTTP 上游连接 / 握手失败 | 降级隔离（同 stdio，计入 skipped），其余上游照常服务 |
| 请求 `Host` 头不在白名单 | rmcp `allowed_hosts` 层拒绝 |
| 既未启用 stdio 也未启用 http | 启动 fail-fast |
| `call_tool` 打到失败上游 / 超时 | 沿用现有：`isError:true` 结构化错误 |

## 7. 测试策略

- **下游 HTTP e2e**：绑 `127.0.0.1:0` 取实际端口 → 用 rmcp Streamable HTTP 客户端连上 → 跑 `search_tools → get_tool_details → call_tool` 全链路（mock 上游提供工具）。
- **下游鉴权**：无 `Authorization` → 401；错误 key → 401；正确 key → 200 / 调用成功；0-key 配置 → 放行。
- **上游 HTTP e2e**：用 `StreamableHttpService` + 现有 `MockUpstream` 起一个 mock HTTP 上游（ephemeral port）→ 配置一个 http 上游指向它 → 断言工具被摄取、`call_tool` 成功、且 mock 收到了预期的鉴权头/自定义头。
- **配置单测**：解析 `[server.http]` / `[[server.http.api_key]]` / http 上游；缺 env → fail-fast；都不启用 → fail-fast。
- **隔离回归**：一个 HTTP 上游崩溃不影响其它上游检索/调用；下游 `tools/list` 恒为 3。
- 全部用 `127.0.0.1` + ephemeral 端口，**无外网依赖**。

## 8. 文档（L1–L4，随代码同提交，纳入双重审查验收）

- **L1**：架构图补 HTTP 双向（上游 http 连接 + 下游 http 暴露）+ 鉴权；更新测试计数与传输能力描述。
- **L2**：`config`（新增 `[server.http]` / http 上游段）、`downstream`（HTTP server + 鉴权层职责）、`upstream`（http connect 职责）。
- **L3**：鉴权层细节（常量时间比较、401 语义）、并发关闭模型（select + cancellation_token）、env 解析与 fail-fast 规则。
- **L4**：新增/变更 API —— `connect_http_upstream`、auth middleware、`HttpConfig` / `ApiKeyConfig` / `UpstreamTransport::Http`、`run_serve` 的并发装配。

## 9. 任务拆分预览（writing-plans 阶段细化为 TDD 步骤）

1. **config schema**：`HttpConfig` / `ApiKeyConfig` / `UpstreamTransport::Http` + env 解析 + fail-fast + 单测。
2. **上游 HTTP transport**：`connect_http_upstream` + `connect_all` 分派；单测 + 占位 e2e 脚手架。
3. **下游 StreamableHttpService 接入**（先不加鉴权）：绑 ephemeral 端口，e2e 全链路通。
4. **Bearer 鉴权层**：401 / 放行 + 常量时间比较 + 鉴权 e2e。
5. **并发进程模型**：stdio + HTTP 共享 state，`select!` 统一关闭。
6. **上游 HTTP e2e**：mock HTTP 上游驱动摄取 + 调用 + 头透传断言。
7. **L1–L4 文档**：补齐并随各 task 提交（每个 task 自带其层级文档，本步做收口校对）。

## 10. 开工前仍需在实现计划里固化的点

- rmcp `StreamableHttpService` 挂载进 axum 的精确写法（`nest_service` vs `route_service`）、tower 鉴权层与 rmcp 服务的类型边界（计划首个相关 task 做最小 spike 固化）。
- `UpstreamTransport::Http` 内嵌 `[[upstream.header]]` 数组在 `#[serde(flatten)]` 内部标签枚举下的 TOML 解析形状（需在 config task 用真实 TOML 验证；若 flatten + 序列不可行，退化为 `headers` inline 表或 `headers_env` 单一来源）。
- mock HTTP 上游用 rmcp `StreamableHttpService` 起真实 axum 还是更轻量的内存桥（影响测试速度与真实度）。
- 鉴权层用 `axum::middleware::from_fn` 还是自定义 `tower::Layer`（取决于读取已解析 keyset 的最简方式）。
