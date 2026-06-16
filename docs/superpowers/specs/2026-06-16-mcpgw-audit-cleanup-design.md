# mcpgw 审计收尾整改设计（最后 5 项 Minor）

> 状态：已定稿，待 writing-plans 细化为实施计划。
> 来源：全量审计 10 项 Minor 的最后 5 项（前 5 项已在两轮整改中合并）。
> 基线（master `6307b8c`）：`cargo test --all-features` 182 passed / 3 ignored，fmt/clippy/build --locked 干净。

## 目标与范围

修复审计剩余的 5 项 Minor，**不新增任何第三方 crate 依赖**（仅 dev-dep 版本对齐）。每项尽量 TDD。

待修：
- N1 检索潜在 panic：`vector.rs` 对空 `Ok` 走 `remove(0)`、`caching.rs` 对偏短返回 `expect` —— 当 `Embedder` 返回的向量数少于输入时会 panic（`crates/retrieval/src/vector.rs`、`crates/retrieval/src/caching.rs`）
- N2 安全：非环回 bind + 空 `api_keys` 静默无鉴权（`crates/mcpgw/src/main.rs`）
- N3 文档：`gateway-lib.md` 误称 `GatewayState::new` 在构造时索引空目录（`docs/L4-api/gateway-lib.md`）
- N4 依赖：downstream dev-dep `reqwest` 0.12 与工作区 0.13 漂移（`crates/downstream/Cargo.toml`）
- N5 测试：`classify` 多个 `error_kind` 与 JsonlSink 写错自愈路径缺测（`crates/downstream`、`crates/observe`）

不变量：纯 Rust、最小依赖；`Embedder` 对外契约（等长、按输入顺序）不变；仅元数据审计不变；现有公开行为不回归。

## N1 — 检索潜在 panic 改为优雅降级

**现状**：
- `vector.rs:104`：`Ok(mut v) => normalize(v.remove(0))` —— 若查询 embedder 返回 `Ok(vec![])`（空），`v.remove(0)` panic。
- `caching.rs:116`：`resolved.get(t).expect("text resolved above")` —— 若内层 embedder 返回的向量数少于 miss 文本数（zip 截断后某 miss 未入 `resolved`），`expect` panic。

二者仅当某 `Embedder` 违反「返回与输入等长」契约时可达；现有 `OpenAiEmbedder` 校验长度不符即 `Err`，故当前不可达——但属潜在健壮性缺口，且与代码他处「优雅降级」风格不一致。

**修复**：
- `vector.rs` 查询路径：把空 `Ok` 与 `Err` 一视同仁——降级到 BM25：
  ```rust
  let qv = match self.embedder.embed(&[query.to_string()]).await {
      Ok(mut v) if !v.is_empty() => normalize(v.remove(0)),
      _ => {
          tracing::warn!("vector query embedding empty/failed; falling back to BM25");
          return self.bm25.search(query, top_k).await;
      }
  };
  ```
- `caching.rs`：内层返回后校验长度，不等即返回 `EmbedError::Provider`（让上层 vector/hybrid 经既有 `Err`→BM25 路径降级），重组 `expect` 从此可证不可达：
  ```rust
  let embedded = self.inner.embed(&miss_texts).await?;
  if embedded.len() != miss_texts.len() {
      return Err(EmbedError::Provider(format!(
          "embedder returned {} vectors for {} inputs",
          embedded.len(),
          miss_texts.len()
      )));
  }
  ```

**测试**（retrieval）：
- `caching` 单测：注入一个返回偏短向量的 `Embedder` → `CachingEmbedder::embed` 返回 `Err(EmbedError::Provider)`（不 panic）。
- `vector` 测试：用一个对单元素查询返回 `Ok(vec![])` 的 embedder 构造 `VectorStrategy`（已索引若干工具）→ `search` 返回 BM25 结果（不 panic）。或退化为直接断言空 `Ok` 路径走降级。

## N2 — 非环回 bind + 空 key 启动期告警

**现状**：`HttpConfig.api_keys` 为空表示「无鉴权（依赖 localhost 绑定）」，但 `bind="0.0.0.0:..."` + 空 `api_keys` 通过 `validate()` → 暴露**无鉴权公网服务**，唯一信号是 `auth=false` 日志。

**修复**（不破坏既有合法配置）：在 `crates/mcpgw/src/main.rs` `run_serve` 装配 HTTP 处，当 `http_enabled` 且**绑定地址非环回/未指定**且 `api_keys` 为空时，发**显著 `tracing::warn!`**（如 "HTTP server is UNAUTHENTICATED and bound to a non-loopback address {bind}; set [[server.http.api_key]] or bind to localhost"）。
- 抽出纯函数便于单测：
  ```rust
  /// True when an HTTP server with NO api keys is bound to a non-loopback (public) address.
  fn unauthenticated_public_bind(bind: &str, has_keys: bool) -> bool {
      if has_keys { return false; }
      match bind.parse::<std::net::SocketAddr>() {
          Ok(addr) => !addr.ip().is_loopback(),
          // Unparseable (e.g. hostname:port) -> can't prove it's loopback; warn to be safe.
          Err(_) => true,
      }
  }
  ```
  注：`0.0.0.0`/`::` 的 `is_loopback()` 为 false（`is_unspecified()` 为 true）→ 视为非环回，告警。`127.0.0.1`/`::1` → 环回，不告警。无法解析（主机名）→ 保守告警。

**测试**（mcpgw 单测）：`unauthenticated_public_bind`：`("0.0.0.0:9000", false)`→true（非环回无 key）；`("127.0.0.1:8970", false)`→false（环回）；`("0.0.0.0:9000", true)`→false（有 key）；`("[::1]:9000", false)`→false（环回 v6）；`("example.com:9000", false)`→true（无法解析，保守告警）。

## N3 — 修正 `gateway-lib.md` 构造期索引误述

**现状**：`docs/L4-api/gateway-lib.md` 称 `GatewayState::new` 用 `build_strategy` 新建策略并**对空 `Catalog` `index`**；实际构造路径从不 `index`（仅 `rebuild_snapshot` 内 `index`），与 `docs/L1-overview.md`「`GatewayState::new` 不在构造时索引」自相矛盾。

**修复**：删除/改写 gateway-lib.md 中「对空 `Catalog` `index`」一句，与 L1 一致（构造时仅装入空快照、首次 `rebuild_snapshot` 前 `search` 返回空）。纯文档改动。

## N4 — `reqwest` dev-dep 版本对齐 0.13

**现状**：`crates/downstream/Cargo.toml` dev-dep `reqwest = "0.12"`，而 `chat`/`embedder`/`rmcp 1.7` 用 0.13 → 测试构建含两套 reqwest + hyper-rustls/tokio-rustls；且该 0.12 dep 冗余（rmcp dev-dep 已拉 0.13）。

**修复**：把 downstream dev-dep 改为 `reqwest = { version = "0.13", default-features = false, features = ["rustls-tls"] }`（与工作区一致）。仅 `crates/downstream/tests/http_server.rs` 用 `reqwest::Client`/`StatusCode`（API 在 0.12→0.13 不变）。

**验证**：`cargo test -p downstream --all-features` 全绿；`cargo tree -p downstream --duplicates 2>/dev/null | grep -c "reqwest v0"` 不再出现两个 reqwest 大版本（理想为 0 重复行）。`Cargo.lock` 变化仅为去重（不应新增第三方 crate）。

## N5 — 补测 `error_kind` 与 JsonlSink 写错自愈

### N5a JsonlSink 写错自愈（observe）

**现状**：`run_writer` 的写失败限频 warn + 保活（自愈）路径无测试。`run_writer(rx: Receiver<String>, file: File)` 直收 `File`，不可注入失败写入器。

**修复**：把 writer 主循环改为对 `impl Write` 泛型，便于注入：
- `spawn_writer` 仍传 `File`；新增内部 `fn run_writer<W: Write>(rx: Receiver<String>, w: W)`（内部自行 `BufWriter::new(w)` 或直接对 `w` 写），`write_line`/`rate_limited_write_error` 相应泛型化或保持私有。
- **测试**：自定义 `Write`，前 N 次 `write` 返回 `Err`、之后成功（并记录成功写入的字节/行）。从 sink 发若干行 → drop sink → writer 退出后断言：writer **未提前退出**、错误后续写仍落地（即「写错自愈」成立）。

> 实现期确认：泛型化不得破坏 `spawn_writer` 对 `File` 的现有行为（append/flush/最终 `sync_all` 仅适用于 `File`；泛型版在非 `File` 测试下跳过 `sync_all` 即可——或仅把**循环体**泛型化、`fsync` 留在 `File` 专用包装里）。以最小改动保持生产路径不变为准。

### N5b `error_kind` 记录级断言（downstream）

**现状**：仅 `tool_not_found` 与成功（`None`）在记录级被断言；`timeout`/`invalid_params` 等未测。

**修复**：在 `crates/downstream/tests/server.rs` 用 `CaptureSink` 补两条**可确定性触发**的：
- `timeout`：attach 一个 `call_timeout` 很短的 mock（`UpstreamHandle::with_call_timeout`），调用 `slow` 工具 → `MetaError::Timeout` → 记录 `outcome=Timeout`、`error_kind="timeout"`。
- `invalid_params`：`call_tool` 不带 `name` → 记录 `outcome=Error`、`error_kind="invalid_params"`。

> `upstream_unavailable`/`upstream_call`/`internal` 难以经公开 API 确定性触发（需上游断连/RPC 错/序列化失败），如实标注**不**新增记录级测试（仍由 `classify` 逻辑显然性 + 既有单测覆盖）。测试 harness 若需短超时 mock，按 `tests/common` 现有 attach 方式加一个带 `with_call_timeout` 的变体（feature 内、不影响生产）。

## 分层文档（DoD）

- N1：`docs/L3-details/retrieval.md`（vector/caching 小节）补「查询 embedder 空返回/偏短返回 → 优雅降级（vector 回退 BM25、caching 返回 `Provider` 错误），不再 panic」。
- N2：`docs/L3-details/mcpgw-cli.md`（serve 鉴权）补「非环回 bind + 空 api_keys 启动期显著告警（不阻断）」。
- N3：即 N3 本身（gateway-lib.md）。
- N5a：`docs/L4-api/observe-audit.md`/`docs/L3-details`：`run_writer` 现对 `impl Write` 泛型（生产仍写 `File`）。
- 若测试计数变化，更新 `docs/L1-overview.md` 测试计数块（按 `cargo test --all-features` 实测）。

## 错误处理与不变量（汇总）

- N1 不改 `Embedder` 对外契约；把违约从 panic 改为 `Err`/降级。
- N2 仅加启动期告警，不改配置 schema、不阻断。
- N4 仅 dev-dep 版本对齐，无新 crate、生产依赖不变。
- N5 仅加测试 + 为可测性做最小泛型化重构，运行时行为不变。

## 实现期需现场确认/可能回退的点

- N1 `vector.rs`：`Ok(mut v) if !v.is_empty()` guard + `_ =>` 合并 Err/空两路降级；确认 `normalize` 入参非空。
- N1 `caching.rs`：长度校验置于 `inner.embed().await?` 之后、插入循环之前；`EmbedError::Provider` 既有变体可用。
- N2：`SocketAddr::parse` 对 `0.0.0.0`/`::`/`127.0.0.1`/`[::1]`/主机名 的判定以单测锁定；告警放在已有 "http server listening" 日志附近。
- N4：若 0.12→0.13 在 `http_server.rs` 有任何 API 差异，最小调整测试代码；确认 `cargo tree --duplicates` 去重生效且 lockfile 不引入新 crate。
- N5a：泛型化范围尽量小——优先只把循环体/`write_line` 泛型化，`fsync` 仍仅对 `File`；保证 `spawn_writer` 生产行为零变化。
- N5b：短超时 mock 的接线方式以 `tests/common` 现状为准；`slow` 工具睡 10s，超时设几十 ms 即可稳定触发 `timeout`。
