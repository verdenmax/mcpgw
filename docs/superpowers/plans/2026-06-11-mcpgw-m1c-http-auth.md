# M1-C: Streamable HTTP 传输 + 静态 API-Key 鉴权 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `mcpgw serve` 既能用 Streamable HTTP 把网关暴露给远程客户端（多 API-Key Bearer 鉴权），又能连接远程 HTTP MCP server 作为上游，stdio 与 HTTP 下游并发共享同一 `GatewayState`。

**Architecture:** 复用现有 `GatewayServer`（rmcp `ServerHandler`），在 `downstream` crate 里用 rmcp `StreamableHttpService` + axum `Router` 装配 HTTP server，外挂一个 tower 鉴权中间件做常量时间 Bearer 校验；上游 HTTP 复用已泛型化的 `UpstreamHandle::connect_with_trigger`，只是把 transport 换成 `StreamableHttpClientTransport`。`mcpgw` 负责启动时解析 env 密钥（fail-fast）、并发跑 stdio + HTTP 两个 server 任务、用 `tokio::select!` 统一关闭。

**Tech Stack:** Rust 2021 / rmcp 1.7（`transport-streamable-http-server`、`transport-streamable-http-client-reqwest`）/ axum 0.8 / tower / subtle（常量时间比较）/ http 1 / tokio。

**Spec:** `docs/superpowers/specs/2026-06-11-mcpgw-m1c-http-auth-design.md`

---

## 已验证的关键 API（实现时直接照抄路径）

```rust
// 下游 server（feature transport-streamable-http-server）
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
let service = StreamableHttpService::new(
    move || Ok(GatewayServer::new(state.clone(), default_top_k)), // Fn() -> Result<S, io::Error>
    Arc::new(LocalSessionManager::default()),                     // Arc<M>；也可写 Default::default()
    StreamableHttpServerConfig::default(),                        // allowed_hosts 默认 [localhost,127.0.0.1,::1]
);
let router = axum::Router::new().nest_service("/mcp", service);   // rmcp 官方测试同款挂载方式
axum::serve(tokio::net::TcpListener::bind(addr).await?, router).await?;

// 上游 client（feature transport-streamable-http-client-reqwest）
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
let cfg = StreamableHttpClientTransportConfig::with_uri(url)
    .auth_header(format!("Bearer {token}"))   // 完整 Authorization 头值
    .custom_headers(map);                      // HashMap<http::HeaderName, http::HeaderValue>
let transport = StreamableHttpClientTransport::from_config(cfg); // 实现 IntoTransport<RoleClient>
```

- `StreamableHttpService<S,M>` 是 `tower_service::Service`，`Response = BoxResponse`、`Error = Infallible` → 可直接 `axum::nest_service`。
- `http::HeaderName` / `http::HeaderValue` 不由 rmcp re-export，需要 `upstream` crate 直接依赖 `http = "1"`。

## File Structure

| 文件 | 职责 | 任务 |
|------|------|------|
| `crates/config/src/lib.rs` | 新增 `HttpConfig`/`ApiKeyConfig`、`ServerConfig.http`、`UpstreamTransport::Http`；让全工作区的 transport 匹配保持穷尽 | T1 |
| `crates/upstream/src/connect.rs` | `build_command` 改为全函数；`connect_http_upstream` + `build_http_client_config`；`connect_all` 按 transport 分派 | T1(总函数化)/T2 |
| `crates/upstream/Cargo.toml` | rmcp `transport-streamable-http-client-reqwest`、`http` 依赖；dev：axum/server-http | T2/T6 |
| `crates/downstream/src/http.rs` (新建) | `build_router(state, top_k, path, api_keys) -> axum::Router` + Bearer 鉴权中间件（常量时间） | T3/T4 |
| `crates/downstream/src/lib.rs` | `pub mod http;` | T3 |
| `crates/downstream/Cargo.toml` | axum/subtle/rmcp server-http；dev：rmcp client-reqwest、reqwest | T3/T4 |
| `crates/downstream/tests/http_server.rs` (新建) | 下游 HTTP e2e + 鉴权 e2e | T3/T4 |
| `crates/mcpgw/src/main.rs` | `resolve_api_keys` + `validate_upstream_http_env`（fail-fast）；`run_serve` 并发 stdio+HTTP + `select!` 关闭 | T5 |
| `crates/mcpgw/Cargo.toml` | axum 依赖；tokio `net`/`signal` feature | T5 |
| `crates/upstream/tests/http_connect.rs` (新建) | 上游 HTTP e2e（mock HTTP 上游 + 头透传断言） | T6 |
| `docs/L1-*`, `docs/L2-*`, `docs/L3-*`, `docs/L4-*` | 分层文档随各任务同提交，T7 收口 | T1–T7 |

---

## Task 1: config schema for HTTP (+ keep workspace compiling)

新增 HTTP 相关配置类型与 `UpstreamTransport::Http` 变体。因为新增枚举变体会破坏全工作区对 `UpstreamTransport` 的穷尽匹配，本任务同时把 `connect.rs::build_command` 改为不再 match 枚举的「全函数」，并修正 config 自身测试里的匹配，使整个工作区保持编译通过、测试全绿。

**Files:**
- Modify: `crates/config/src/lib.rs`
- Modify: `crates/upstream/src/connect.rs`（仅为保持穷尽匹配做最小改动；HTTP 连接逻辑在 T2）

- [ ] **Step 1: 写失败测试（config 解析 [server.http] / api_key / http 上游）**

在 `crates/config/src/lib.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
    #[test]
    fn parses_server_http_section_with_api_keys() {
        let cfg = Config::from_toml_str(
            r#"
            [server]
            stdio = true
            [server.http]
            enabled = true
            bind = "0.0.0.0:9000"
            path = "/gw"
            [[server.http.api_key]]
            name = "claude"
            env  = "MCPGW_KEY_CLAUDE"
            [[server.http.api_key]]
            name = "cursor"
            env  = "MCPGW_KEY_CURSOR"
            "#,
        )
        .unwrap();
        let http = cfg.server.http.expect("http section present");
        assert!(http.enabled);
        assert_eq!(http.bind, "0.0.0.0:9000");
        assert_eq!(http.path, "/gw");
        assert_eq!(http.api_keys.len(), 2);
        assert_eq!(http.api_keys[0].name, "claude");
        assert_eq!(http.api_keys[0].env, "MCPGW_KEY_CLAUDE");
    }

    #[test]
    fn server_http_defaults_when_omitted_or_partial() {
        // 整个 [server.http] 省略 -> None。
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.server.http.is_none());
        // 只给 enabled -> bind/path 用默认，api_key 空。
        let cfg = Config::from_toml_str("[server.http]\nenabled = true\n").unwrap();
        let http = cfg.server.http.unwrap();
        assert!(http.enabled);
        assert_eq!(http.bind, "127.0.0.1:8970");
        assert_eq!(http.path, "/mcp");
        assert!(http.api_keys.is_empty());
    }

    #[test]
    fn parses_http_upstream_with_bearer_and_headers() {
        let cfg = Config::from_toml_str(
            r#"
            [[upstream]]
            name = "remote"
            transport = "http"
            url = "https://example.com/mcp"
            bearer_env = "REMOTE_BEARER"
            headers = { "X-Api-Version" = "REMOTE_VER" }
            "#,
        )
        .unwrap();
        let u = &cfg.upstreams[0];
        assert_eq!(u.call_timeout_ms, 30_000); // 默认仍生效
        match &u.transport {
            UpstreamTransport::Http { url, bearer_env, headers } => {
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(bearer_env.as_deref(), Some("REMOTE_BEARER"));
                assert_eq!(headers.get("X-Api-Version").map(String::as_str), Some("REMOTE_VER"));
            }
            _ => panic!("expected http transport"),
        }
    }

    #[test]
    fn http_upstream_url_must_not_be_blank() {
        let err = Config::from_toml_str(
            "[[upstream]]\nname=\"r\"\ntransport=\"http\"\nurl=\"  \"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn server_http_rejects_unknown_field() {
        // HttpConfig 无 flatten -> deny_unknown_fields 生效。
        assert!(Config::from_toml_str("[server.http]\nbogus = 1\n").is_err());
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p config`
Expected: 编译失败（`HttpConfig`/`ApiKeyConfig`/`UpstreamTransport::Http` 未定义；现有单臂 match 因新变体非穷尽）。

- [ ] **Step 3: 实现 config 类型与校验**

在 `crates/config/src/lib.rs`：

3a. 给 `ServerConfig` 增加 `http` 字段：

```rust
/// `[server]` section: which downstream transport(s) to serve.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Serve the 3 meta-tools over a stdio MCP server.
    pub stdio: bool,
    /// Optional Streamable HTTP server. Omitted -> `None` (HTTP disabled).
    pub http: Option<HttpConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            stdio: true,
            http: None,
        }
    }
}

/// `[server.http]`: Streamable HTTP server settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// Start the HTTP server. Defaults to false (must opt in).
    pub enabled: bool,
    /// Bind address. Defaults to localhost; use a tunnel/reverse proxy for public exposure.
    pub bind: String,
    /// Mount path for the MCP endpoint.
    pub path: String,
    /// Accepted API keys. Empty -> no auth (relies on localhost binding).
    #[serde(rename = "api_key")]
    pub api_keys: Vec<ApiKeyConfig>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1:8970".into(),
            path: "/mcp".into(),
            api_keys: Vec::new(),
        }
    }
}

/// One accepted API key. The secret is referenced by env var name only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyConfig {
    /// Label for logs/observability. NEVER the key value.
    pub name: String,
    /// Name of the env var holding the key secret.
    pub env: String,
}
```

3b. 给 `UpstreamTransport` 增加 `Http` 变体：

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum UpstreamTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env_passthrough: Vec<String>,
    },
    /// Remote HTTP MCP server (Streamable HTTP). Auth values referenced by env name only.
    Http {
        /// Endpoint URL, e.g. "https://example.com/mcp".
        url: String,
        /// Optional env var holding a bearer token -> sent as `Authorization: Bearer <token>`.
        #[serde(default)]
        bearer_env: Option<String>,
        /// Custom headers: header-name -> env-var-name holding the header value.
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
    },
}
```

3c. 在 `validate()` 的 upstream 循环里（`seen.insert` 之后）追加 HTTP url 非空校验：

```rust
            if let UpstreamTransport::Http { url, .. } = &u.transport {
                if url.trim().is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "upstream {:?}: http url must not be empty",
                        u.name
                    )));
                }
            }
```

- [ ] **Step 4: 修正因新变体而非穷尽的匹配（保持工作区编译）**

4a. `crates/config/src/lib.rs` 的两个现有测试里有单臂 `match &u.transport { UpstreamTransport::Stdio { .. } => ... }`（`parses_stdio_upstreams`、`parses_explicit_call_timeout_through_flatten`）。给它们各加一条：

```rust
            UpstreamTransport::Http { .. } => unreachable!("stdio fixture"),
```

4b. `crates/upstream/src/connect.rs`：把 `build_command` 改成不再 match 枚举的全函数，并让 `connect_stdio_upstream` 负责解构 `Stdio`：

```rust
/// Build the child command for a stdio upstream (env allow-list applied).
pub(crate) fn build_command(
    command: &str,
    args: &[String],
    env_passthrough: &[String],
) -> tokio::process::Command {
    tokio::process::Command::new(command).configure(|c| {
        c.args(args);
        c.env_clear();
        for key in env_passthrough {
            if let Ok(val) = std::env::var(key) {
                c.env(key, val);
            }
        }
    })
}
```

`connect_stdio_upstream` 开头改为解构后调用：

```rust
pub async fn connect_stdio_upstream(
    cfg: &UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError> {
    let UpstreamTransport::Stdio {
        command,
        args,
        env_passthrough,
    } = &cfg.transport
    else {
        return Err(UpstreamError::Connect {
            server: cfg.name.clone(),
            source: "connect_stdio_upstream called on a non-stdio upstream".into(),
        });
    };
    let cmd = build_command(command, args, env_passthrough);
    // ...（其余不变：TokioChildProcess::new(cmd) -> connect_with_trigger -> 超时 -> with_call_timeout）
```

4c. `crates/upstream/src/connect.rs` 的测试 `build_command_applies_env_allowlist` 改为解构后调用：

```rust
        let cfg = stdio_cfg(vec!["MCPGW_TEST_ALLOWED".to_string()]);
        let UpstreamTransport::Stdio {
            command,
            args,
            env_passthrough,
        } = &cfg.transport
        else {
            unreachable!()
        };
        let cmd = build_command(command, args, env_passthrough);
```

> 注：此时 `connect_all` 仍只调用 `connect_stdio_upstream`，因此配置了 `transport = "http"` 的上游会被降级 skip（返回 "non-stdio" 错误）。HTTP 连接在 T2 接上。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p config && cargo build -p upstream && cargo test -p upstream`
Expected: 全部 PASS（config 新测试通过；upstream 编译通过、原有测试仍绿）。

- [ ] **Step 6: 文档（L4 config + L3 config）**

- `docs/L4-api/config-lib.md`：新增 `HttpConfig`/`ApiKeyConfig` 字段说明、`ServerConfig.http`、`UpstreamTransport::Http { url, bearer_env, headers }`（注明 headers 是「头名→env 变量名」的内联表，密钥只经 env 引用）。
- `docs/L3-details/config.md`：补一段「HTTP 上游 headers 用内联表而非 `[[upstream.header]]`，以规避 `#[serde(flatten)]` + 内部标签枚举对数组表的解析限制（spec §10 的回退选项）」。

- [ ] **Step 7: 提交**

```bash
git add crates/config/src/lib.rs crates/upstream/src/connect.rs docs/L4-api/config-lib.md docs/L3-details/config.md
git commit -m "feat(config): HTTP server + http upstream config schema (M1-C T1)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: 上游 HTTP transport（connect_http_upstream + connect_all 分派）

实现连接远程 HTTP MCP 上游：从 env 解析 bearer/headers，构造 `StreamableHttpClientTransportConfig`，复用泛型 `connect_with_trigger`。`connect_all` 按 transport 分派。env→config 的纯逻辑用单元测试覆盖；真正的网络连接由 T6 的 e2e 覆盖。

**Files:**
- Modify: `crates/upstream/Cargo.toml`（rmcp client-reqwest feature + `http` 依赖）
- Modify: `crates/upstream/src/connect.rs`
- Modify (workspace dep): `Cargo.toml`（加 `http = "1"`）

- [ ] **Step 1: 加依赖**

`Cargo.toml`（workspace `[workspace.dependencies]`）追加：

```toml
http = "1"
```

`crates/upstream/Cargo.toml` 的 rmcp features 追加 `transport-streamable-http-client-reqwest`，并加 `http`：

```toml
rmcp = { workspace = true, features = ["client", "server", "macros", "transport-child-process", "transport-io", "transport-streamable-http-client-reqwest"] }
http = { workspace = true }
```

- [ ] **Step 2: 写失败测试（env→client config 的纯逻辑）**

在 `crates/upstream/src/connect.rs` 的 `#[cfg(test)] mod tests` 末尾追加。`build_http_client_config` 是不做 I/O 的纯函数（只读 env + 组装 config），便于单测：

```rust
    use config::UpstreamTransport;

    fn http_cfg(bearer_env: Option<&str>, headers: &[(&str, &str)]) -> UpstreamConfig {
        UpstreamConfig {
            name: "remote".to_string(),
            call_timeout_ms: 1_000,
            transport: UpstreamTransport::Http {
                url: "http://127.0.0.1:1/mcp".to_string(),
                bearer_env: bearer_env.map(str::to_string),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            },
        }
    }

    #[test]
    fn build_http_client_config_sets_bearer_and_headers_from_env() {
        std::env::set_var("MCPGW_T2_BEARER", "sekret");
        std::env::set_var("MCPGW_T2_VER", "2024-01");
        let cfg = http_cfg(Some("MCPGW_T2_BEARER"), &[("X-Api-Version", "MCPGW_T2_VER")]);
        let UpstreamTransport::Http { url, bearer_env, headers } = &cfg.transport else {
            unreachable!()
        };
        let client_cfg = build_http_client_config(&cfg.name, url, bearer_env, headers).unwrap();
        assert_eq!(client_cfg.auth_header.as_deref(), Some("Bearer sekret"));
        assert_eq!(
            client_cfg
                .custom_headers
                .get(&http::HeaderName::from_static("x-api-version"))
                .map(|v| v.to_str().unwrap()),
            Some("2024-01")
        );
    }

    #[test]
    fn build_http_client_config_missing_env_is_error() {
        let cfg = http_cfg(Some("MCPGW_T2_DEFINITELY_MISSING"), &[]);
        let UpstreamTransport::Http { url, bearer_env, headers } = &cfg.transport else {
            unreachable!()
        };
        let err = build_http_client_config(&cfg.name, url, bearer_env, headers).unwrap_err();
        assert!(matches!(err, UpstreamError::Connect { .. }));
    }

    #[test]
    fn build_http_client_config_no_auth_is_ok() {
        let cfg = http_cfg(None, &[]);
        let UpstreamTransport::Http { url, bearer_env, headers } = &cfg.transport else {
            unreachable!()
        };
        let client_cfg = build_http_client_config(&cfg.name, url, bearer_env, headers).unwrap();
        assert!(client_cfg.auth_header.is_none());
        assert!(client_cfg.custom_headers.is_empty());
    }
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p upstream build_http_client_config`
Expected: 编译失败（`build_http_client_config` 未定义）。

- [ ] **Step 4: 实现 build_http_client_config + connect_http_upstream + 分派**

`crates/upstream/src/connect.rs` 顶部 imports 追加：

```rust
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
```

实现纯函数（放在 `connect_http_upstream` 之前）：

```rust
/// Resolve env-referenced auth into an rmcp client transport config. No I/O beyond env reads.
/// A referenced-but-missing env var is a hard error (auth must not be silently dropped).
fn build_http_client_config(
    name: &str,
    url: &str,
    bearer_env: &Option<String>,
    headers: &std::collections::HashMap<String, String>,
) -> Result<StreamableHttpClientTransportConfig, UpstreamError> {
    let mut cfg = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    if let Some(env_name) = bearer_env {
        let token = std::env::var(env_name).map_err(|_| UpstreamError::Connect {
            server: name.to_string(),
            source: format!("missing env {env_name:?} for bearer_env").into(),
        })?;
        cfg = cfg.auth_header(format!("Bearer {token}"));
    }
    if !headers.is_empty() {
        let mut custom = std::collections::HashMap::new();
        for (hname, env_name) in headers {
            let val = std::env::var(env_name).map_err(|_| UpstreamError::Connect {
                server: name.to_string(),
                source: format!("missing env {env_name:?} for header {hname:?}").into(),
            })?;
            let hn = http::HeaderName::from_bytes(hname.as_bytes()).map_err(|e| {
                UpstreamError::Connect {
                    server: name.to_string(),
                    source: Box::new(e),
                }
            })?;
            let hv = http::HeaderValue::from_str(&val).map_err(|e| UpstreamError::Connect {
                server: name.to_string(),
                source: Box::new(e),
            })?;
            custom.insert(hn, hv);
        }
        cfg = cfg.custom_headers(custom);
    }
    Ok(cfg)
}

/// Connect one HTTP upstream. Handshake bounded by `call_timeout_ms`, same as stdio.
pub async fn connect_http_upstream(
    cfg: &UpstreamConfig,
    trigger: Option<RebuildTrigger>,
) -> Result<UpstreamHandle, UpstreamError> {
    let UpstreamTransport::Http {
        url,
        bearer_env,
        headers,
    } = &cfg.transport
    else {
        return Err(UpstreamError::Connect {
            server: cfg.name.clone(),
            source: "connect_http_upstream called on a non-http upstream".into(),
        });
    };
    let client_cfg = build_http_client_config(&cfg.name, url, bearer_env, headers)?;
    let transport = StreamableHttpClientTransport::from_config(client_cfg);
    let connect = UpstreamHandle::connect_with_trigger(&cfg.name, transport, trigger);
    let handle =
        match tokio::time::timeout(Duration::from_millis(cfg.call_timeout_ms), connect).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                return Err(UpstreamError::Timeout {
                    server: cfg.name.clone(),
                })
            }
        };
    Ok(handle.with_call_timeout(Duration::from_millis(cfg.call_timeout_ms)))
}
```

把 `connect_all` 的循环体改为按 transport 分派：

```rust
    for cfg in upstreams {
        let result = match &cfg.transport {
            UpstreamTransport::Stdio { .. } => {
                connect_stdio_upstream(cfg, Some(trigger.clone())).await
            }
            UpstreamTransport::Http { .. } => {
                connect_http_upstream(cfg, Some(trigger.clone())).await
            }
        };
        match result {
            Ok(handle) => {
                registry.insert(Arc::new(handle));
                summary.connected.push(cfg.name.clone());
            }
            Err(e) => {
                tracing::warn!(upstream = %cfg.name, error = %e, "connect failed; skipping");
                summary.skipped.push((cfg.name.clone(), e.to_string()));
            }
        }
    }
```

文件顶部 `use config::{UpstreamConfig, UpstreamTransport};` 已存在；确认 `UpstreamTransport` 在导入项里。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p upstream`
Expected: 新增 3 个单测 PASS；原有 upstream 测试仍绿。

- [ ] **Step 6: 文档（L4 upstream-connect + L3 upstream）**

- `docs/L4-api/upstream-connect.md`：新增 `connect_http_upstream` 与 `build_http_client_config` 的签名/语义（bearer_env→`Authorization: Bearer`，headers 头名→env；缺 env=硬错误；握手超时同 stdio）；更新 `connect_all` 说明（按 transport 分派，HTTP 失败同样降级）；记录 `build_command` 已改为全函数签名。
- `docs/L3-details/upstream.md`：补「HTTP 上游复用泛型 `connect_with_trigger`（M1-B.2 把签名泛化为 `IntoTransport<RoleClient>` 的收益），stdio 与 http 共用同一 list_changed / 超时管线」。

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml crates/upstream/Cargo.toml crates/upstream/src/connect.rs docs/L4-api/upstream-connect.md docs/L3-details/upstream.md
git commit -m "feat(upstream): connect HTTP upstreams via Streamable HTTP client (M1-C T2)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: 下游 HTTP server（先不加鉴权）+ e2e

在 `downstream` crate 里用 `StreamableHttpService` + axum 装配一个 HTTP server router，复用 `GatewayServer`。本任务先不接鉴权（T4 加），用 e2e 验证「绑端口→连上→search/details/call 全链路」。

**Files:**
- Modify: `crates/downstream/Cargo.toml`
- Create: `crates/downstream/src/http.rs`
- Modify: `crates/downstream/src/lib.rs`（加 `pub mod http;`）
- Create: `crates/downstream/tests/http_server.rs`

- [ ] **Step 1: 加依赖**

`crates/downstream/Cargo.toml`：

```toml
[dependencies]
gateway = { path = "../gateway" }
metatools = { path = "../metatools" }
rmcp = { workspace = true, features = ["server", "transport-io", "transport-streamable-http-server"] }
serde_json = { workspace = true }
axum = "0.8"

[dev-dependencies]
upstream = { path = "../upstream", features = ["testkit"] }
retrieval = { path = "../retrieval" }
rmcp = { workspace = true, features = ["client", "server", "transport-io", "transport-streamable-http-client-reqwest"] }
tokio = { workspace = true, features = ["full"] }
serde_json = { workspace = true }
```

- [ ] **Step 2: 写失败的 e2e 测试**

`crates/downstream/tests/http_server.rs`（新建）：

```rust
use std::sync::Arc;

use gateway::GatewayState;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::json;
use upstream::connection::UpstreamHandle;
use upstream::testkit::MockUpstream;

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

async fn attach_mock(state: &GatewayState, name: &str) {
    let (server_io, client_io) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        if let Ok(svc) = MockUpstream::new().serve(server_io).await {
            let _ = svc.waiting().await;
        }
    });
    let handle = UpstreamHandle::connect(name, client_io).await.unwrap();
    state.registry().insert(Arc::new(handle));
    state.rebuild_snapshot().await.unwrap();
}

/// Bind the gateway's HTTP router on an ephemeral port; return the bound addr.
async fn spawn_http_gateway(state: Arc<GatewayState>, api_keys: Vec<String>) -> String {
    let router = downstream::http::build_router(state, 8, "/mcp", api_keys);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn http_gateway_serves_search_details_call() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec![]).await;

    let client = ()
        .serve(StreamableHttpClientTransport::from_uri(url))
        .await
        .unwrap();

    // list_tools -> exactly 3 meta-tools.
    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["call_tool", "get_tool_details", "search_tools"]);

    // search -> details -> call.
    let r = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query":"echo"}))),
        )
        .await
        .unwrap();
    assert!(r.content[0].as_text().unwrap().text.contains("mock__echo"));

    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "mock__echo", "arguments": {"text": "hi"}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("hi"));

    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p downstream --test http_server`
Expected: 编译失败（`downstream::http::build_router` 不存在）。

- [ ] **Step 4: 实现 build_router（无鉴权版）**

`crates/downstream/src/lib.rs` 顶部模块声明区追加：

```rust
pub mod http;
```

`crates/downstream/src/http.rs`（新建）：

```rust
//! HTTP serving of the gateway's 3 meta-tools over Streamable HTTP (axum + rmcp
//! `StreamableHttpService`). Bearer auth (M1-C T4) is layered on when keys are configured.

use std::sync::Arc;

use gateway::GatewayState;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use crate::GatewayServer;

/// Build the axum router that serves the 3 meta-tools at `path`. `api_keys` is accepted
/// now for a stable signature; Bearer enforcement is added in T4.
pub fn build_router(
    state: Arc<GatewayState>,
    default_top_k: usize,
    path: &str,
    _api_keys: Vec<String>,
) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(GatewayServer::new(state.clone(), default_top_k)),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    axum::Router::new().nest_service(path, service)
}
```

> rmcp `StreamableHttpServerConfig::default()` 的 `allowed_hosts` 默认 `[localhost,127.0.0.1,::1]`，对 `127.0.0.1` e2e 放行。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p downstream`
Expected: 新 e2e + 原有 server.rs 测试全 PASS。

- [ ] **Step 6: 文档（L4 downstream + L2 downstream）**

- 新建 `docs/L4-api/downstream-http.md`：`build_router(state, default_top_k, path, api_keys) -> axum::Router` 的签名/职责，注明用 rmcp `StreamableHttpService` + `nest_service` 挂载、`allowed_hosts` 默认 localhost。
- `docs/L2-components/downstream.md`：新增「HTTP server（Streamable HTTP）职责」小节，说明它复用同一 `GatewayServer`，只是换了 transport。

- [ ] **Step 7: 提交**

```bash
git add crates/downstream/Cargo.toml crates/downstream/src/lib.rs crates/downstream/src/http.rs crates/downstream/tests/http_server.rs docs/L4-api/downstream-http.md docs/L2-components/downstream.md
git commit -m "feat(downstream): serve meta-tools over Streamable HTTP (M1-C T3)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: Bearer 鉴权层（401 / 放行 + 常量时间比较）

给 `build_router` 接上鉴权中间件：配置了 ≥1 个 key 时校验 `Authorization: Bearer <k>`（常量时间比较），否则 401；0 个 key 时放行。

**Files:**
- Modify: `crates/downstream/Cargo.toml`（加 `subtle`；dev 加 `reqwest`）
- Modify: `crates/downstream/src/http.rs`
- Modify: `crates/downstream/tests/http_server.rs`

- [ ] **Step 1: 加依赖**

`crates/downstream/Cargo.toml`：`[dependencies]` 加 `subtle = "2"`；`[dev-dependencies]` 加用于断言 401 的 HTTP 客户端：

```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
```

- [ ] **Step 2: 写失败的鉴权测试**

在 `crates/downstream/tests/http_server.rs` 追加。复用 Step 同名 helper（`attach_mock` / `spawn_http_gateway`）：

```rust
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

async fn post_init(url: &str, bearer: Option<&str>) -> reqwest::StatusCode {
    let mut req = reqwest::Client::new()
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json");
    if let Some(b) = bearer {
        req = req.header("Authorization", format!("Bearer {b}"));
    }
    req.body(INIT_BODY).send().await.unwrap().status()
}

#[tokio::test]
async fn http_auth_rejects_missing_and_wrong_key() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec!["good-key".to_string()]).await;

    assert_eq!(post_init(&url, None).await, reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(post_init(&url, Some("bad")).await, reqwest::StatusCode::UNAUTHORIZED);
    // 正确 key 不应是 401（会进入 MCP 协议层，返回 2xx）。
    assert_ne!(post_init(&url, Some("good-key")).await, reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn http_auth_allows_valid_key_full_flow() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    attach_mock(&state, "mock").await;
    let url = spawn_http_gateway(state, vec!["good-key".to_string()]).await;

    let cfg = StreamableHttpClientTransportConfig::with_uri(url).auth_header("Bearer good-key");
    let client = ()
        .serve(StreamableHttpClientTransport::from_config(cfg))
        .await
        .unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 3);
    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p downstream --test http_server http_auth`
Expected: FAIL（无 key 时当前是放行 200，而非 401）。

- [ ] **Step 4: 实现鉴权中间件，接入 build_router**

`crates/downstream/src/http.rs` 顶部 imports 追加：

```rust
use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;
```

追加中间件与辅助函数：

```rust
#[derive(Clone)]
struct ApiKeys(Arc<Vec<String>>);

/// True if `presented` equals any configured key. Per-key compare is constant-time for
/// equal-length inputs (length mismatch short-circuits, leaking only length). No early
/// return across the key set.
fn key_authorized(keys: &[String], presented: &[u8]) -> bool {
    let mut matched = 0u8;
    for k in keys {
        matched |= u8::from(k.as_bytes().ct_eq(presented).unwrap_u8());
    }
    matched == 1
}

fn presented_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

async fn require_api_key(State(keys): State<ApiKeys>, req: Request, next: Next) -> Response {
    match presented_bearer(&req) {
        Some(k) if key_authorized(&keys.0, k.as_bytes()) => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}
```

把 `build_router` 改为按 `api_keys` 是否为空决定挂层（去掉参数名前的下划线）：

```rust
pub fn build_router(
    state: Arc<GatewayState>,
    default_top_k: usize,
    path: &str,
    api_keys: Vec<String>,
) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(GatewayServer::new(state.clone(), default_top_k)),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service(path, service);
    if api_keys.is_empty() {
        router
    } else {
        router.layer(from_fn_with_state(
            ApiKeys(Arc::new(api_keys)),
            require_api_key,
        ))
    }
}
```

> `ct_eq` 来自 `subtle::ConstantTimeEq`：`&[u8]` 长度不同会短路返回 `Choice(0)`（仅泄露长度，可接受），长度相同则做常量时间逐字节比较。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p downstream`
Expected: 4 个 http 测试 + 原有测试全 PASS。

- [ ] **Step 6: 文档（L4 downstream-http + L3 downstream）**

- `docs/L4-api/downstream-http.md`：补鉴权语义——`api_keys` 非空时挂 `require_api_key` 层；无/错 Bearer→401；常量时间比较；空 keyset→放行。
- `docs/L3-details/downstream.md`：补「鉴权层细节：Bearer 提取、`subtle::ConstantTimeEq` 常量时间比较、401 不回显期望值；keyset 为空时依赖 localhost 绑定 + rmcp allowed_hosts」。

- [ ] **Step 7: 提交**

```bash
git add crates/downstream/Cargo.toml crates/downstream/src/http.rs crates/downstream/tests/http_server.rs docs/L4-api/downstream-http.md docs/L3-details/downstream.md
git commit -m "feat(downstream): Bearer API-key auth layer for HTTP server (M1-C T4)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: mcpgw 并发进程模型（stdio + HTTP）+ env 解析 fail-fast

让 `mcpgw serve` 启动时解析所有 env 密钥（缺失即 fail-fast），按配置并发跑 stdio 与 HTTP 两个 server 任务，用 `tokio::select!` + `ctrl_c` 统一关闭。

**Files:**
- Modify: `crates/mcpgw/Cargo.toml`
- Modify: `crates/mcpgw/src/main.rs`

- [ ] **Step 1: 加依赖**

`crates/mcpgw/Cargo.toml`：`[dependencies]` 加 `axum`，并给 tokio 加 `net` + `signal`：

```toml
axum = "0.8"
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync", "net", "signal"] }
```

- [ ] **Step 2: 写失败测试（env 解析）**

`resolve_api_keys` / `validate_upstream_http_env` 是可单测的纯逻辑（只读 env）。在 `crates/mcpgw/src/main.rs` 的 `#[cfg(test)] mod tests` 追加：

```rust
    #[test]
    fn resolve_api_keys_reads_env_and_fails_fast_on_missing() {
        std::env::set_var("MCPGW_T5_KEY", "abc");
        let cfg = config::Config::from_toml_str(
            "[server.http]\nenabled = true\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_T5_KEY\"\n",
        )
        .unwrap();
        assert_eq!(resolve_api_keys(&cfg).unwrap(), vec!["abc".to_string()]);

        let cfg = config::Config::from_toml_str(
            "[server.http]\nenabled = true\n[[server.http.api_key]]\nname=\"a\"\nenv=\"MCPGW_T5_MISSING\"\n",
        )
        .unwrap();
        assert!(resolve_api_keys(&cfg).is_err());
    }

    #[test]
    fn resolve_api_keys_empty_when_no_http() {
        let cfg = config::Config::default_from_empty();
        assert!(resolve_api_keys(&cfg).unwrap().is_empty());
    }

    #[test]
    fn validate_upstream_http_env_fails_fast_on_missing_bearer() {
        let cfg = config::Config::from_toml_str(
            "[[upstream]]\nname=\"r\"\ntransport=\"http\"\nurl=\"http://x/mcp\"\nbearer_env=\"MCPGW_T5_NO_SUCH\"\n",
        )
        .unwrap();
        assert!(validate_upstream_http_env(&cfg).is_err());
    }
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p mcpgw resolve_api_keys`
Expected: 编译失败（函数未定义）。

- [ ] **Step 4: 实现 env 解析 + 并发 run_serve**

`crates/mcpgw/src/main.rs` 顶部 imports 追加：

```rust
use config::UpstreamTransport;
```

新增两个 fail-fast 解析函数（放在 `prepare_state` 之前）：

```rust
/// Resolve every `[[server.http.api_key]]` secret from its env var. Fail-fast on any
/// missing env (returns the offending field/env name, never the value).
fn resolve_api_keys(cfg: &config::Config) -> Result<Vec<String>, String> {
    let Some(http) = &cfg.server.http else {
        return Ok(Vec::new());
    };
    let mut keys = Vec::with_capacity(http.api_keys.len());
    for k in &http.api_keys {
        let secret = std::env::var(&k.env)
            .map_err(|_| format!("api_key {:?}: env {:?} is not set", k.name, k.env))?;
        keys.push(secret);
    }
    Ok(keys)
}

/// Verify every env referenced by an HTTP upstream (bearer + headers) is present, so a
/// missing credential fails startup rather than silently degrading to a 401 loop.
fn validate_upstream_http_env(cfg: &config::Config) -> Result<(), String> {
    for u in &cfg.upstreams {
        if let UpstreamTransport::Http {
            bearer_env,
            headers,
            ..
        } = &u.transport
        {
            if let Some(env_name) = bearer_env {
                if std::env::var(env_name).is_err() {
                    return Err(format!(
                        "upstream {:?}: bearer_env {:?} is not set",
                        u.name, env_name
                    ));
                }
            }
            for (hname, env_name) in headers {
                if std::env::var(env_name).is_err() {
                    return Err(format!(
                        "upstream {:?}: header {:?} env {:?} is not set",
                        u.name, hname, env_name
                    ));
                }
            }
        }
    }
    Ok(())
}
```

把 `run_serve` 改为并发模型（替换现有 `run_serve` 函数体；保留顶部的 tracing 初始化）：

```rust
async fn run_serve(cfg: config::Config) -> Result<(), String> {
    use rmcp::transport::stdio;
    use rmcp::ServiceExt;

    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let http_enabled = cfg.server.http.as_ref().is_some_and(|h| h.enabled);
    if !cfg.server.stdio && !http_enabled {
        return Err(
            "no server transport enabled (set [server].stdio or [server.http].enabled)".into(),
        );
    }

    // Fail-fast: resolve/verify every env-referenced secret before connecting anything.
    let api_keys = resolve_api_keys(&cfg)?;
    validate_upstream_http_env(&cfg)?;

    let (state, rx) = prepare_state(&cfg).await?;
    tokio::spawn(gateway::run_rebuild_worker((*state).clone(), rx));

    // Pre-bind the HTTP listener (fail-fast on bind errors) before entering select!.
    let http_bound = if http_enabled {
        let h = cfg.server.http.as_ref().unwrap();
        let listener = tokio::net::TcpListener::bind(&h.bind)
            .await
            .map_err(|e| format!("bind {:?}: {e}", h.bind))?;
        tracing::info!(bind = %h.bind, path = %h.path, auth = api_keys.len() > 0, "http server listening");
        let router =
            downstream::http::build_router(state.clone(), cfg.retrieval.top_k, &h.path, api_keys);
        Some((listener, router))
    } else {
        None
    };

    let stdio_enabled = cfg.server.stdio;
    let state_for_stdio = state.clone();
    let top_k = cfg.retrieval.top_k;

    tokio::select! {
        res = async {
            let server = downstream::GatewayServer::new(state_for_stdio, top_k);
            let service = server.serve(stdio()).await.map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())
        }, if stdio_enabled => {
            res?;
            tracing::info!("stdio client disconnected; shutting down");
        }
        res = async {
            let (listener, router) = http_bound.unwrap();
            axum::serve(listener, router).await.map_err(|e| e.to_string())
        }, if http_enabled => {
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c; shutting down");
        }
    }

    // Best-effort graceful shutdown of upstream children.
    for name in state.registry().server_names() {
        if let Some(handle) = state.registry().remove(&name) {
            if let Ok(h) = Arc::try_unwrap(handle) {
                h.shutdown().await;
            }
        }
    }
    Ok(())
}
```

> `tokio::select!` 的 `if <precondition>` 为假时不会构造对应 `async` future，所以 `http_bound.unwrap()` 只在 `http_enabled` 时执行；stdio 分支同理。任一分支完成（stdio EOF / HTTP server 退出 / Ctrl-C）即触发整体关闭，其余 future 被 drop。

- [ ] **Step 5: 运行测试 + 全量构建，确认通过**

Run: `cargo test -p mcpgw && cargo build`
Expected: 新 env 单测 PASS；现有 `run_serve_builds_initial_snapshot_with_no_upstreams` / `cli_parses_serve_subcommand` 仍绿；全工作区编译通过。

- [ ] **Step 6: 文档（L4 mcpgw-main + L3 mcpgw-cli + L2 mcpgw-cli）**

- `docs/L4-api/mcpgw-main.md`：新增 `resolve_api_keys` / `validate_upstream_http_env`；更新 `run_serve`（并发 stdio+HTTP、select! 关闭、fail-fast 顺序、HTTP listener 预绑定）。
- `docs/L3-details/mcpgw-cli.md`：补「并发关闭模型（select! over stdio/http/ctrl_c）、至少启用一种传输、env 密钥启动期解析」。
- `docs/L2-components/mcpgw-cli.md`：更新 `serve` 子命令描述（支持 stdio 与/或 HTTP）。

- [ ] **Step 7: 提交**

```bash
git add crates/mcpgw/Cargo.toml crates/mcpgw/src/main.rs docs/L4-api/mcpgw-main.md docs/L3-details/mcpgw-cli.md docs/L2-components/mcpgw-cli.md
git commit -m "feat(mcpgw): concurrent stdio+HTTP serve with env fail-fast (M1-C T5)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6: 上游 HTTP e2e（mock HTTP 上游 + 头透传断言）

起一个真实的 HTTP MCP 上游（`StreamableHttpService` + `MockUpstream`），外挂一个记录请求头的中间件；用 `connect_http_upstream` 连上，断言工具被摄取、`call_tool` 成功，且 mock 收到了预期的 `Authorization` 头。

**Files:**
- Modify: `crates/upstream/Cargo.toml`（dev-deps + 新 test target）
- Create: `crates/upstream/tests/http_connect.rs`

- [ ] **Step 1: 加 dev 依赖与 test target**

`crates/upstream/Cargo.toml`：

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["full"] }
axum = "0.8"
rmcp = { workspace = true, features = ["client", "server", "macros", "transport-io", "transport-streamable-http-server", "transport-streamable-http-client-reqwest"] }

[[test]]
name = "http_connect"
required-features = ["testkit"]
```

> 已有的 `[[test]] name = "integration"` 与 `[[bin]] name = "mock-stdio"` 保留不动。

- [ ] **Step 2: 写失败的 e2e 测试**

`crates/upstream/tests/http_connect.rs`（新建）：

```rust
//! e2e: connect to a real HTTP MCP upstream (rmcp StreamableHttpService + MockUpstream),
//! verifying tool ingestion, call forwarding, and that auth headers reach the upstream.

use std::sync::{Arc, Mutex};

use axum::{
    extract::{Request, State},
    middleware::{from_fn_with_state, Next},
    response::Response,
};
use catalog::Catalog;
use config::{UpstreamConfig, UpstreamTransport};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use upstream::connect::connect_http_upstream;
use upstream::testkit::MockUpstream;

type Headers = Arc<Mutex<Vec<(String, String)>>>;

async fn record_headers(State(store): State<Headers>, req: Request, next: Next) -> Response {
    if let Some(v) = req.headers().get("authorization").and_then(|v| v.to_str().ok()) {
        store
            .lock()
            .unwrap()
            .push(("authorization".to_string(), v.to_string()));
    }
    next.run(req).await
}

/// Spawn a mock HTTP MCP upstream; return (url, recorded-headers store).
async fn spawn_mock_http_upstream() -> (String, Headers) {
    let store: Headers = Arc::new(Mutex::new(Vec::new()));
    let service = StreamableHttpService::new(
        || Ok(MockUpstream::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(store.clone(), record_headers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{addr}/mcp"), store)
}

fn http_cfg(name: &str, url: &str) -> UpstreamConfig {
    std::env::set_var("MCPGW_T6_BEARER", "topsecret");
    UpstreamConfig {
        name: name.to_string(),
        call_timeout_ms: 5_000,
        transport: UpstreamTransport::Http {
            url: url.to_string(),
            bearer_env: Some("MCPGW_T6_BEARER".to_string()),
            headers: std::collections::HashMap::new(),
        },
    }
}

#[tokio::test]
async fn connects_http_upstream_ingests_and_calls_with_auth_header() {
    let (url, headers) = spawn_mock_http_upstream().await;
    let cfg = http_cfg("remote", &url);

    let handle = connect_http_upstream(&cfg, None).await.expect("connect");

    // Tools are ingested (namespaced).
    let mut catalog = Catalog::new();
    handle.ingest_into(&mut catalog).await.unwrap();
    assert!(catalog.get("remote__echo").is_some(), "echo should be ingested");

    // call_tool forwards and returns the echoed text.
    let mut args = serde_json::Map::new();
    args.insert("text".to_string(), serde_json::json!("hi-http"));
    let result = handle.call_tool("echo", Some(args)).await.unwrap();
    assert!(result.content[0].as_text().unwrap().text.contains("hi-http"));

    // The upstream saw our Authorization: Bearer header.
    let recorded = headers.lock().unwrap();
    assert!(
        recorded
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer topsecret"),
        "upstream should have received the bearer header, got: {recorded:?}"
    );

    handle.shutdown().await;
}
```

> `MockUpstream` 是 `#[tool]` 宏生成的 handler，需实现 `Clone`（`StreamableHttpService` 的 factory 每会话 `Ok(MockUpstream::new())` 直接新建，不依赖 Clone）。若 `MockUpstream::new()` 不是 `Fn` 闭包可用形式，改用 `|| Ok(MockUpstream::new())` 已满足 `Fn() -> Result<S, io::Error>`。

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p upstream --features testkit --test http_connect`
Expected: 先确认它能编译并实际跑（若 `connect_http_upstream` 行为正确，此步可能直接 PASS——这是验收 T2 连接路径的 e2e）。如未通过，按报错修 T2 的连接逻辑。

- [ ] **Step 4: （按需）修复连接路径**

若 Step 3 失败，依据错误（握手、IntoTransport 绑定、header 透传）调整 `crates/upstream/src/connect.rs` 的 `connect_http_upstream`，重跑直至通过。无需新增实现即通过时跳过本步。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p upstream --features testkit`
Expected: `http_connect` + 原有 `integration` + 库内单测全 PASS。

- [ ] **Step 6: 文档（L3 upstream）**

- `docs/L3-details/upstream.md`：补「HTTP 上游 e2e 用 rmcp `StreamableHttpService` 起真实 axum mock 上游 + 头记录中间件验证鉴权头透传」。

- [ ] **Step 7: 提交**

```bash
git add crates/upstream/Cargo.toml crates/upstream/tests/http_connect.rs docs/L3-details/upstream.md
git commit -m "test(upstream): e2e HTTP upstream ingest/call + auth-header forwarding (M1-C T6)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 7: L1/L2 文档收口 + 全量验证

各任务已随代码提交了对应 L3/L4（及部分 L2）文档；本任务做系统级 L1/L2 收口、跨层一致性校对、测试计数更新，并跑全量验证。

**Files:**
- Modify: `docs/L1-overview.md`
- Modify: `docs/L2-components/config.md`, `docs/L2-components/upstream.md`
- Modify: `docs/README.md`（如有约定/排除项需更新）

- [ ] **Step 1: L1 总览更新**

`docs/L1-overview.md`：
- 架构图/描述补 HTTP 双向：上游可连远程 HTTP MCP server；下游可经 Streamable HTTP 暴露（含 Bearer 鉴权）。
- 传输能力表：upstream = stdio + http；downstream = stdio + http（并发）。
- 更新测试总数（运行 `cargo test --all-features 2>&1 | grep "test result:"` 累加后填入）。

- [ ] **Step 2: L2 组件文档更新**

- `docs/L2-components/config.md`：补 `[server.http]` 段与 http 上游段（headers 内联表 + env 引用约定）。
- `docs/L2-components/upstream.md`：补「http transport：连远程 HTTP MCP，复用泛型连接路径」。

- [ ] **Step 3: 跨层一致性校对**

逐项核对 spec 的每个范围条目（§1–§8）都能指到一个任务/代码点；核对 L1↔L2↔L3↔L4 对 HTTP/鉴权的描述无矛盾（端口默认 `127.0.0.1:8970`、path 默认 `/mcp`、headers 是「头名→env」内联表、密钥只经 env、cancellation 仍延后）。修正任何漂移。

- [ ] **Step 4: 全量验证**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
Expected: fmt 干净、clippy 无告警、所有测试 PASS（含 testkit 门控的 `integration` 与 `http_connect`）。

- [ ] **Step 5: 提交**

```bash
git add docs/L1-overview.md docs/L2-components/config.md docs/L2-components/upstream.md docs/README.md
git commit -m "docs: L1/L2 sync for M1-C HTTP transport + auth (M1-C T7)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 收尾（全部任务完成后）

1. 派发最终整体 code review（subagent-driven-development 的 final reviewer），核对 spec 覆盖、并发/关闭正确性、鉴权安全性（常量时间、不泄露、localhost 默认）、无回归。
2. 处理 review 的 blocking 项（如有）。
3. 使用 superpowers:finishing-a-development-branch 把 `feat/m1c-http-auth` 合回 master（`--no-ff`，本地），删分支，更新 roadmap（M1-C done → M1 全部完成）。

## 实现期需现场确认/可能回退的点（spec §10）

- `axum::Router::nest_service(path, StreamableHttpService)` 的 body/error 类型边界：已用 rmcp 官方测试同款写法（`Response = BoxResponse`、`Error = Infallible`），若 axum 0.8 版本细节有出入，按编译错误微调（必要时 `route_service`）。
- `StreamableHttpClientTransport::from_config(..)` 是否实现 `IntoTransport<RoleClient>`：由 T6 e2e 实证；若签名不符，改用 rmcp 提供的对应 `serve` 入口。
- `subtle::ConstantTimeEq for [u8]` 的长度短路语义：以 T4 测试为准；如版本差异导致 panic，改为先比长度再 `ct_eq` 等长切片。
- headers 内联表在 `#[serde(flatten)]` + 内部标签枚举下的解析：以 T1 测试 `parses_http_upstream_with_bearer_and_headers` 为准；若失败，回退为 `headers_env`（单一 env 持 JSON）方案并同步更新 spec/文档。
