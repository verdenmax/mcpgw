# mcpgw dashboard 审计修复（pass-1）设计

日期：2026-06-17
状态：已批准设计（待写实施计划）

## 背景

只读可视化面板（dashboard，子系统 A）合并后做了一次独立全子系统审计：**0 Critical / 0 Important / 2 Minor**。
核心逻辑无可利用 bug/注入/panic/竞态/泄密。两项 Minor 都是「无鉴权 localhost」既定设计上的纵深防御，本 spec 修这两项。

## N1 — Host 头校验（防 DNS rebinding，security/Minor）

**现状**：dashboard 无鉴权，安全模型是「绑 loopback，故只有本机可达」。但**没有 Host 头校验**：运维浏览器访问的恶意网页可把自身域名 DNS 重绑到 `127.0.0.1`，再对 `http://attacker-host:8971/api/*` 发**同源** `fetch`，读到工具/上游名、指标、以及（开了 `trace_queries` 时）**query 原文**。SOP 在 rebinding 下不设防。

**修复（仅 loopback 绑定才校验）**：
- 在 `dashboard` crate 加一层 axum 中间件 + 一个**可单测的纯函数** `host_is_local(host: Option<&str>) -> bool`：
  - 从 `Host` 头取主机名（处理 `host:port` 与 IPv6 的 `[::1]:port`），去端口；`localhost`（大小写不敏感）或可解析为 `IpAddr` 且 `is_loopback()` 即 local。
  - 缺失/不可解析/非 local → 不 local。
- `build_dashboard_router(state, enforce_loopback_host: bool)` 新增布尔参数。`enforce_loopback_host == true` 时挂中间件：
  请求 Host 非 local → **403**（不进任何 handler）；否则放行。`false` 时不挂中间件。
- 装配（`mcpgw serve`）按 `cfg.dashboard.bind` 是否 loopback 计算该布尔：复用现有 `unauthenticated_public_bind(&bind, false)`（绑非 loopback 时返回 true），故 `enforce_loopback_host = !unauthenticated_public_bind(&cfg.dashboard.bind, false)`。
  - **loopback 绑定（默认）**：enforce=true → Host 必须是 localhost 名 → 堵住 rebinding。
  - **非 loopback 绑定**（运维显式暴露、已有告警、自管反代/鉴权）：enforce=false → 跳过校验，**不破坏既有合法部署**。

精准命中威胁模型（DNS rebinding 只对 loopback 服务成立），零破坏。

**DNS rebinding 为何被堵**：恶意页面 `http://attacker.com:8971` 即使解析到 `127.0.0.1`，浏览器发出的 `Host` 头仍是 `attacker.com` → `host_is_local` 为 false → 403。

**测试**：纯函数 `host_is_local`——`Some("127.0.0.1:8971")`/`Some("localhost")`/`Some("[::1]:8971")`/`Some("LOCALHOST")` → true；`Some("evil.com:8971")`/`Some("192.168.1.5")`/`None` → false。（中间件本身是薄封装；端到端由现有 e2e 覆盖：reqwest 发 `127.0.0.1:port` Host，loopback 绑定下仍 200。）

## N2 — 截断 trace 里的 query 长度（trace/Minor）

**现状**：discovery ring 只限**条数**（`trace_buffer`，默认 500）不限**字节**，逐条存完整 client query 原文，故常驻内存 ≈ `trace_buffer × 最大 query 长`。反复发超大 query（需开 `trace_queries`）放大内存。

**修复**：在 `downstream` 构造 `DiscoveryRecord` 处（`discovery_record_for_search`）把 `query` 截断到
`MAX_TRACE_QUERY_CHARS = 2048` **字符**（按字符边界，UTF-8 安全，不 panic）。ring 内存上限随之 = `trace_buffer × ≤2048 字符`，有界。
工具名已受 ingest 上限（`MAX_TOOL_TEXT_BYTES`）约束，无需再截。只影响 opt-in 的发现追踪显示，不改任何检索/调用行为。

**测试**：超 2048 字符的 query → record 的 `query` 恰被截到 2048 字符；短 query 原样；含多字节字符的 query 在 2048 字符边界附近截断不 panic、不产生半个码点。

## 不做的事（YAGNI）

- 不给 dashboard 加鉴权（仍 localhost-only；这是子系统 A 的既定取舍）。
- 不引入 `allowed_hosts` 可配置项（仅按 bind 是否 loopback 自动决定）。
- 不截断工具名（已有界）。
- 不引入新依赖。

## 验证

- `cargo fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features`、
  `cargo test -p mcpgw --test dashboard -- --ignored`、`cargo build --locked` 全绿。
- 同步分层文档（dashboard L3/L4 的安全/进程模型、`mcpgw-main` L4 的装配、L1 测试计数）。

## 交付

按本仓库一贯工作流：subagent 实现 + 每 task spec+质量双重审查、折叠 nit、最终整分支 code-review、`--no-ff` 本地合并、
复测、删分支、推送 origin；修复后 findings id 20/21 置 `fixed`。
