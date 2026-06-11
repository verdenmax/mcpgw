# L4 — `crates/downstream/src/http.rs` API

源文件：`crates/downstream/src/http.rs`。把同一个 `GatewayServer`（3 个固定元工具）经 **Streamable HTTP**
transport 暴露：用 rmcp `StreamableHttpService` 装进 axum `Router`。鉴权（M1-C T4 的 Bearer 层）在配置了
API-Key 时再叠加；本文件先不接鉴权。

## `fn build_router`
```rust
pub fn build_router(
    state: Arc<gateway::GatewayState>,
    default_top_k: usize,
    path: &str,
    _api_keys: Vec<String>,
) -> axum::Router
```

**职责**：构造一个把 3 个元工具挂在 `path`（如 `/mcp`）下的 axum `Router`。

- 用 `StreamableHttpService::new(factory, session_manager, config)` 构造服务：
  - `factory`：`move || Ok(GatewayServer::new(state.clone(), default_top_k))`，签名为
    `Fn() -> Result<S, std::io::Error>`，每个会话克隆共享 `state`（仅克隆内部 `Arc`）复用同一份网关状态。
  - `session_manager`：`Arc::new(LocalSessionManager::default())`（进程内会话表）。
  - `config`：`StreamableHttpServerConfig::default()`，其 `allowed_hosts` 默认
    `[localhost, 127.0.0.1, ::1]`，对本机 `127.0.0.1` e2e 放行。
- `StreamableHttpService<S, M>` 实现 `tower_service::Service`（`Response = BoxResponse`、
  `Error = Infallible`），故可直接 `axum::Router::new().nest_service(path, service)` 挂载。

**参数 `_api_keys`**：现在接受只为保持签名稳定；T4 才在此叠加 Bearer 校验中间件。下划线前缀使其在未用时
保持 clippy 干净。无错误（返回 `axum::Router`）。

**起服务**：调用方用 `axum::serve(TcpListener::bind(addr).await?, build_router(...)).await` 起监听。
客户端用 `StreamableHttpClientTransport::from_uri("http://{addr}/mcp")` 连接。

## 依赖

- 内部：`crate::GatewayServer`、`gateway::GatewayState`。
- 外部：`rmcp`（feature `transport-streamable-http-server`：`StreamableHttpService` /
  `StreamableHttpServerConfig` / `session::local::LocalSessionManager`）、`axum`（0.8，http 1 / hyper 1）。

> 同一 `GatewayServer` 也可经 stdio 暴露，见 L4：[lib](./downstream-lib.md)。组件视角见 L2：
> [downstream](../L2-components/downstream.md)。
