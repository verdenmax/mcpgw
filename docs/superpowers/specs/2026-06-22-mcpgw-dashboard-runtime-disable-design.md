# Dashboard 运行时临时禁用 + Bearer 鉴权设计（写子系统 B）

> 状态：已设计待评审 · 日期 2026-06-22 · 子系统 B（dashboard 第一个写能力）

## 目标

给 dashboard 加**运行时临时禁用**能力：运维可在不重启进程、不改主配置的前提下，**临时禁用**一个抖动/危险的
上游（namespace）或单个工具，让它从下游 MCP 客户端的 `search_tools`/`get_tool_details`/`call_tool` 中**隐藏**；
随时可再启用、秒级恢复（保留上游连接）。禁用状态**跨重启持久化**。所有变更经 **Bearer token 鉴权**；
"查看当前禁用了什么"则**开放只读**。

这是 dashboard 路线图（`2026-06-17-mcpgw-dashboard-drilldown-admin-design.md`）里的 **M4（写子系统 #1）**，
也是 M5（在线改配）的鉴权前置。

## 背景与现状（已核实）

- `GatewayState`（`crates/gateway/src/lib.rs`）：`snapshot: Arc<ArcSwap<GatewaySnapshot>>` + `registry:
  UpstreamRegistry` + `strategy_name` + `backends` + `rebuild_lock: Mutex<()>` + `last_summary`。
  `rebuild_snapshot()` 并发 ingest 每个上游 → 建 `Catalog` → `strategy.index(&catalog)` → 原子 `store` 新快照
  （build-then-swap，读无锁；`rebuild_lock` 串行重建、last-store-wins）。
- 三个元工具是 `&GatewaySnapshot` 上的纯函数（`crates/metatools/src/tools.rs`）：`search_tools`（走 strategy）、
  `get_tool_details(snap, name)` = `snap.catalog().get(name)`、`call_tool(snap, registry, name, args)` =
  `catalog().get(name)` 解析 `ToolDef` → `registry.get(def.server)` → `handle.call_tool(...)`；
  catalog 无该名 → `MetaError::ToolNotFound`。
- 两个下游传输（`crates/downstream/src/lib.rs` stdio、`crates/downstream/src/http.rs` HTTP）各持
  `Arc<GatewayState>`，经 `self.state.snapshot()` 调元工具。`main.rs` 构造**一个** `Arc<GatewayState>` 共享给
  stdio + HTTP 下游 + dashboard。
- dashboard `AppState`（`crates/dashboard/src/api.rs`）持 `gateway: Arc<GatewayState>`（**完整、可重建**）；
  现有 13 个 `/api/*` **全是 `get`、纯只读**；无任何鉴权（默认绑 `127.0.0.1`）。
- HTTP api-key Bearer 鉴权**已有现成实现**：`crates/downstream/src/http.rs` 用 `subtle::ConstantTimeEq`，
  `key_authorized`（累加式 `ct_eq` 常量时间）、`presented_bearer`（解析 `Authorization: Bearer <token>`，
  scheme 大小写不敏感、空 token 视为未提供）、`require_api_key` 中间件、按需挂载。dashboard 复用同一模式。
- 密钥配置惯例：`ApiKeyConfig{ name, env }`，注释"the secret is referenced by env var name only"；
  安全基线"密钥仅环境变量、永不入文件/日志"。

## 非目标（YAGNI）

- 不做在线改配/热重载（那是 M5，另立项）；本设计的"持久化"只是禁用集这一项运行时状态的落盘，**不碰
  `mcpgw.toml`、不引入 `toml_edit`**。
- 不做用户系统/多账号/RBAC：写鉴权就是**单一共享 admin token**。
- 不做 WebSocket/SSE：前端继续 3s 轮询。
- 不改 `metatools` 与 `downstream` 的签名/逻辑（隐藏式语义经现有代码路径自然达成，见下）。
- 不引入"ToolDisabled"独立错误：隐藏式语义下，被禁用项对客户端表现为 `ToolNotFound`（正确——客户端不该知道
  它存在）。

---

## 架构与集成

**核心洞见**：`disable/enable` 是 admin 动作（非热路径），handler 可**先改禁用集、再 `await
rebuild_snapshot()`、再返回 200**。rebuild 完成后新快照已不含被禁用项 → 后续 `search_tools` 搜不到、
`get_tool_details`/`call_tool` 经 `catalog().get()` 自然返回 None（即 `ToolNotFound`）。**隐藏式语义因此经现有
代码路径达成，metatools 与 downstream 零改动。**
唯一缺口：禁用瞬间已 in-flight 的那一次调用会跑完——任何"先检查后调用"都无法避免，文档标注即可。

`DisableSet` 放进 **`gateway` crate**（dashboard 已依赖 gateway，main 也依赖；gateway 绝不反向依赖 dashboard →
无依赖环）。`GatewayState` 加 `disabled: Arc<DisableSet>`，默认空集 → **未配置时行为与今天完全一致**。

```
                       mcpgw serve （单进程，一个 Arc<GatewayState>）
   ┌─────────────────────────────────────────────────────────────────────┐
   │  GatewayState { snapshot: ArcSwap<…>, registry, disabled: DisableSet }│
   │        ▲ rebuild_snapshot() 读 disabled：                            │
   │        │   · 被禁用 upstream → 整个跳过 ingest（抖动上游不再拖慢重建）│
   │        │   · 被禁用单工具   → upsert 时跳过                          │
   └──┬──────────────────┬───────────────────────────────┬───────────────┘
      │ 共享 Arc          │ 共享 Arc                       │ 共享 Arc
      ▼                   ▼                                ▼
  downstream(stdio)   downstream(HTTP)              dashboard AppState
  self.state.snapshot()→metatools                  (持同一 GatewayState)
  （零改动，自然看不到禁用项）                       ├─ GET /api/disabled（开放）读 disabled
                                                    └─ POST /api/admin/…（Bearer）改 disabled
                                                          → 持久化 → await gateway.rebuild_snapshot()
```

**两个相互独立的配置开关**（解耦"读应用"与"写鉴权"）：

- `[dashboard].disabled_state_path: Option<String>` → 有则**加载并应用**持久化禁用集（重启仍生效）+
  `GET /api/disabled` 反映之。
- `[dashboard].admin_token_env: Option<String>` → 有则**额外**让 `/api/admin/*` 变更 API 通过 Bearer 中间件
  放行（未配则中间件统一返 404，见鉴权一节）。
- 二者都配 = 既能改又能持久化（目标态）；只配 path 不配 token = 禁用仍跨重启生效但只能手改文件；
  都不配 = 完全等于今天的纯只读面板。

**备选（未采纳）**：在 metatools 层过滤而非 rebuild——可免去每次 toggle 的 rebuild、快照保留全量，但要改三个
元工具签名 + 两个下游、过滤逻辑分散，且 search 可能 under-fill `top_k`。rebuild 方案更聚合，且"禁用抖动上游 →
rebuild 不再等它"是额外红利；rebuild 成本可控（连接保留、BM25 默认廉价、向量走 CachingEmbedder 缓存命中）。

---

## 数据模型与持久化

### `DisableSet`（`crates/gateway/src/disable.rs`，新文件）

```rust
pub struct DisableSet {
    inner: RwLock<DisabledState>,   // 同步 RwLock；toggle 罕见、读多写极少
    path: Option<PathBuf>,          // None = 纯内存（不落盘）
}
struct DisabledState { upstreams: BTreeSet<String>, tools: BTreeSet<String> } // BTree → 天然有序

impl DisableSet {
    /// 启动期：有 path 且文件存在则读入；解析失败 → 空集 + warn（绝不 panic）。
    pub fn load_or_new(path: Option<PathBuf>) -> Self;

    /// 过滤判定（rebuild_snapshot 调用，读锁，无分配热点）。
    pub fn is_upstream_disabled(&self, name: &str) -> bool;
    pub fn is_tool_disabled(&self, qualified: &str) -> bool;

    /// 变更（admin handler 调用）：返回 changed:bool；仅 changed 时持久化。
    pub fn disable_upstream(&self, name: &str) -> bool;
    pub fn enable_upstream(&self, name: &str) -> bool;
    pub fn disable_tool(&self, qualified: &str) -> bool;
    pub fn enable_tool(&self, qualified: &str) -> bool;

    /// GET /api/disabled + 持久化共用的有序快照。
    pub fn snapshot(&self) -> DisabledSnapshot; // { upstreams: Vec<String>, tools: Vec<String> }（有序）
}

#[derive(Serialize, Deserialize)]
pub struct DisabledSnapshot { pub upstreams: Vec<String>, pub tools: Vec<String> }
```

### 状态文件

`disabled_state_path` 指向的 JSON（约定文件名 `mcpgw-disabled.json`，**须显式在 config 配置、无自动默认**，
须 gitignore）：

```json
{ "upstreams": ["flaky-server"], "tools": ["github__delete_repo"] }
```

- **原子写**（每次 changed 变更后，best-effort）：写同目录临时文件 `*.tmp.<pid>` → `flush`+`fsync` → `rename`
  覆盖正式文件（rename 同盘原子，崩溃绝不留半截）。写失败只 `warn!`、**不回滚内存状态、不阻断 API**
  （下次成功 toggle 整体重写）；为不阻塞 axum async 线程，文件写经 `spawn_blocking`（文件极小、动作罕见）。
- **加载语义**：文件缺失 → 空集（正常非错误）；坏 JSON / 坏 UTF-8 → 空集 + `warn!`（自愈，不让坏文件挡启动）。
  加载**保留**所有名字（即便该 upstream/tool 已从 config 移除）——陈旧名天然 inert（过滤判定永不命中），
  `enable` 可清除；不在加载期做存在性裁剪（tools 是动态的，无法可靠校验）。

### `rebuild_snapshot` 改动（surgical，`crates/gateway/src/lib.rs`）

- ingest 阶段：`for name in server_names()` 增加 `if self.disabled.is_upstream_disabled(&name) { continue; }`
  —— 被禁用上游**连 `tools/list` 都不发**（抖动上游不再拖慢 rebuild）。
- upsert 阶段：`for tool in local.iter()` 对 `is_tool_disabled(&tool.qualified_name())` 的跳过 `upsert`。
- 注入：给 `GatewayState` 加 `with_disabled(...)`（或现有构造旁加 setter）+ `disabled() -> &Arc<DisableSet>`
  getter；`serve` 在**首次 rebuild 之前**注入已加载的 DisableSet，使启动即生效。

### config 新增字段（`crates/config/src/lib.rs` 的 `DashboardConfig`，皆 `Option`、默认 `None` → 行为不变）

- `disabled_state_path: Option<String>`（**无自动默认**：`None` = 纯内存、重启即清；要兑现"跨重启持久化"
  须显式配置一个路径，如 `mcpgw-disabled.json`，与 `audit.path`/`trace_path` 的显式路径惯例一致）
- `admin_token_env: Option<String>`（env 变量名，**不存 token 本身**；`serve` 启动期 `std::env::var` 解析，
  缺失/空 → fail-fast 报错退出，与 `[[server.http.api_key]]` 一致）

---

## API 面与鉴权

### 开放端点（始终挂载，无 token）

| 方法 | 路径 | 返回 |
|---|---|---|
| GET | `/api/disabled` | `Json(DisabledSnapshot)`（有序；空集即 `{ "upstreams": [], "tools": [] }`） |

### Admin 端点（始终挂载，全程经 `require_admin_token` 中间件）

| 方法 | 路径 | 动作 |
|---|---|---|
| POST | `/api/admin/upstreams/{name}/disable` | 校验 `name` ∈ 配置上游 → 否则 404；改集 → 持久化 → `await rebuild` → `200 Json(DisabledSnapshot)` |
| POST | `/api/admin/upstreams/{name}/enable` | 同上（enable 不校验存在性，幂等） |
| POST | `/api/admin/tools/{name}/disable` | 校验 `name` ∈ 当前 catalog → 否则 404；其余同上 |
| POST | `/api/admin/tools/{name}/enable` | 移除即可，幂等 |

- **幂等优先于存在性校验**：handler 先查禁用集——**已是目标态 → 直接回 200 + 当前集、跳过 rebuild 与存在性
  校验**（故二次 disable 一个已隐藏工具不会误 404）；仅当是"真正的新变更"时才做存在性校验（upstream ∈ 配置
  上游、tool ∈ 当前 catalog，否则 404），再改集 → 持久化 → rebuild。返回整集让前端一个往返即刷新。
- **串行化**：handler 外包一把 `tokio::Mutex` 串行"改集 + 持久化 + rebuild"，避免并发 toggle 引发 rebuild
  风暴；`rebuild_snapshot` 本身又有 `rebuild_lock` 且在 rebuild 时读"当前"集 → 最终收敛一致。
- 端点计数：dashboard 路由 13 → **18**（+1 开放 GET + 4 admin POST）。

### 鉴权中间件 `require_admin_token`（`crates/dashboard/src/admin.rs`，复用 http.rs 模式）

```rust
// State: AdminToken(Option<Arc<str>>) —— 启动期由 admin_token_env 解析得到
async fn require_admin_token(State(tok): State<AdminToken>, req: Request, next: Next) -> Response {
    match tok.0.as_deref() {
        None => StatusCode::NOT_FOUND.into_response(),       // 未配置 → 404，不泄露存在性
        Some(expected) => match presented_bearer(&req) {     // 复用 Bearer 解析
            Some(t) if t.as_bytes().ct_eq(expected.as_bytes()).into() => next.run(req).await,
            _ => StatusCode::UNAUTHORIZED.into_response(),   // 配了但缺/错 → 401
        }
    }
}
```

> **为何"始终挂载 + 中间件返 404"而非"不挂载"**：SPA fallback 会把未知路径回退成 `index.html`；若 admin 路由
> 不挂载，`POST /api/admin/…` 会落到 fallback 拿到 200 + HTML。始终挂载、由中间件在未配 token 时返 404，
> 才能保证干净的 404 且不向 POST 吐 HTML。`presented_bearer` 在 dashboard 内重实现一份（~6 行，保持 crate 解耦）。

### 错误一览

- 未配 token → admin 全 404；配了但缺/错 Bearer → 401。
- 未知上游名 / 未知工具（disable）→ 404。
- 持久化写失败 → 操作仍 200（warn）。
- `GET /api/disabled` 永远 200。

### 装配（`crates/mcpgw/src/main.rs` serve）

- 解析 `admin_token_env` → `Option<Arc<str>>`（缺失/空 fail-fast）。
- `DisableSet::load_or_new(disabled_state_path)` → `Arc`，注入 `GatewayState`（**首次 rebuild 之前**）+ 随
  `AppState` 共享给 dashboard。
- `build_dashboard_router` 新签名带 `admin_token: AdminToken` 与 disabled 句柄（经 `state.gateway.disabled()`
  即可，无需单独传）。

---

## 前端（Svelte 5，沿用既有模式，token 仅存内存）

- `lib/admin.svelte.js`（新）：模块级 `export const admin = $state({ token: "" })` —— **只在内存、刷新即失，
  绝不 localStorage**；`adminPost(path)` 用 `Authorization: Bearer ${admin.token}` 发 POST。
- `lib/api.js`：加 `postJSON(path, token)`。
- **About/Settings 加「Admin / 写访问」段**：用裸 bool `admin_enabled`（= 是否配了 `admin_token_env`，
  **镜像现有 `http_auth` 隐私模式、不含密钥**）显示 enabled/disabled；enabled 时给一个 `<input type=password>`
  绑 `admin.token` + 提示"仅内存保存，刷新即失"。
- **Upstreams 列表/详情**（`Upstreams.svelte` / `UpstreamDetail.svelte`）：从 `/api/disabled` 读集合 → 给被禁用
  上游打 `disabled` 徽标；`admin.token` 非空时每行出 Disable/Enable 按钮 → `adminPost` → 成功后 `refreshNow`。
- **Tools 列表/详情**（`Tools.svelte` / `ToolDetail.svelte`）：同上；**另加「Disabled tools」小区**（隐藏式下被
  禁用工具已从 catalog 消失，需从 `/api/disabled.tools` 单独列出，各带 Enable 按钮）。
- 未输入 token → 完全等于今天的只读视图（不显示开关）。
- 需 `AboutInfo` 增 `dashboard.admin_enabled: bool`（后端 `about.rs` 顺带改 + 隐私测试覆盖）。

---

## 数据流

**读（开放）**：
```
前端轮询 ──▶ GET /api/disabled ──▶ DisableSet.snapshot() ──▶ Upstreams/Tools 渲染 disabled 徽标 + Disabled tools 区
```

**写（Bearer）**：
```
前端开关 ──Bearer──▶ POST /api/admin/.../disable ──(admin Mutex)──▶ DisableSet.disable_*(持久化)
                                                                  └─▶ await gateway.rebuild_snapshot()
                                                                          └─▶ ArcSwap 原子换 ──▶ 搜索/路由即时隐藏
```

---

## 测试策略（Rust 为主；按偏好重 corner + 核心；可派专测 subagent）

- `DisableSet` 单元（gateway）：disable/enable 幂等 + changed；`is_*_disabled`；snapshot 有序；
  **持久化往返**（写 → 读相等）；原子写不留 `.tmp`、正式文件合法；不可写路径 best-effort（warn + 内存仍生效、
  仍返 changed）；缺文件 → 空；坏 JSON → 空且不 panic；陈旧名 inert。
- `rebuild_snapshot` 过滤（gateway）：禁用上游其工具消失且**不 ingest**（mock 上游断言未发 `tools/list`）；
  禁用单工具消失；enable 恢复；禁用不存在上游名在 rebuild 中 inert。
- 中间件（dashboard `admin.rs`）：未配 → 404；配了 + 缺/空/错 Bearer → 401；对 → 放行；scheme 大小写；
  常量时间路径被走到。
- handlers（dashboard）：disable 上游 → `GET /api/disabled` 反映；未知上游 → 404；tool disable 存在性 → 404；
  enable 幂等；返回整 `DisabledSnapshot`；持久化文件被写。
- **e2e 冒烟**（`#[ignore]`，仿现有）：serve + admin token + mock 上游 → POST disable 上游 → 客户端
  `search_tools` 不再含其工具、`call_tool` → `ToolNotFound` → POST enable → 复现；无 token POST → 401。
- corner：URL 编码特殊字符 name；工具名含 `__`；重复 disable 不触发双 rebuild（仍 200）；并发 toggle 收敛；
  空 `admin_token_env` → 启动 fail-fast；about 隐私（token 值绝不出现在 `/api/about`，扩展既有隐私测试）。
- 构建：`npm run build` 复现 `ui/dist`；新组件编译通过。

## 文档（L1–L4，随代码同提交）

- **L1-overview**：dashboard 段——把"已完成的纯只读"修订为"**默认只读；可选 opt-in 的运行时禁用写子系统 B**
  （Bearer 鉴权、`GET /api/disabled` 开放）"；gateway 段补 `DisableSet` + rebuild 过滤。
- **L2 dashboard.md**：修订"纯只读"不变量为"**默认只读；配置后可运行时禁用**"；加 `admin.rs`、`DisableSet`
  集成、新端点；端点计数 13 → 18。
- **L2/L3 gateway.md**：`DisableSet` 字段 + rebuild 过滤 + 跳过禁用上游 ingest。
- **L2/L3/L4 config**：`[dashboard].admin_token_env` / `disabled_state_path`。
- **L4 dashboard.md**：`admin.rs` + 新端点 API；**L4 mcpgw-main.md**：装配/ fail-fast 接线。

## 交付流程（仓库惯例）

subagent-driven-development，**每 task 跑完整 spec + 质量双重审查**、折叠 nit、整分支 `code-review`、`--no-ff`
本地合并、复测、删分支、推 origin；门禁 `cargo fmt --all --check` + `cargo clippy --all-targets --all-features
-D warnings` + `cargo test --all-features` + `cargo build --locked` + `npm run build` 复现 `dist`。
分层文档 L1–L4 同步、L1 测试计数更新。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| 打破"面板纯只读"不变量 | 默认仍只读；写能力 opt-in（须配 `admin_token_env`）；读/写端点分离；L1/L2 文档同步修订不变量措辞 |
| 持久化写失败 / 半截文件 | best-effort + 原子 temp→fsync→rename；失败只 warn 不阻断；下次 toggle 整体重写 |
| 禁用瞬间 in-flight 调用漏过一次 | 文档明示（任何先检查后调用都无法避免）；await rebuild 关闭后续窗口 |
| 每次 toggle 触发 rebuild 偏重 | 连接保留、被禁用上游不再 ingest、向量走缓存；toggle 是罕见 admin 动作 |
| token 泄露 | env 引用、绝不入文件/日志/`/api/about`；常量时间比较；前端仅内存存 token |
| 依赖环 | `DisableSet` 落 gateway crate；gateway 不依赖 dashboard |
