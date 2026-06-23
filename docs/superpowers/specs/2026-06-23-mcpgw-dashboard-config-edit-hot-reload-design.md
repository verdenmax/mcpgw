# Dashboard 在线改配 + 热重载设计（写子系统 C / M5）

> 状态：已设计待评审 · 日期 2026-06-23 · 子系统 C（dashboard 第二个写能力，复用 M4 的 Bearer 鉴权）

## 目标

让运维在浏览器里**编辑整份 `mcpgw.toml`**：严格校验（结构 + 所有 env 引用可解析）后**原子落盘**（带 `.bak`
备份），并对 **`[[upstream]]` 增/删/改做热重载**（秒级生效，无需重启）；其余字段（retrieval/server/dashboard/
audit/各类密钥）**可编辑、可落盘，但需重启才生效**，返回"需重启"提示。所有读写经 **M4 的 Bearer 鉴权**。

这是 dashboard 路线图（`2026-06-17-mcpgw-dashboard-drilldown-admin-design.md`）里的 **M5（写子系统 C）**，
依赖 M4 的鉴权。

## 背景与现状（已核实）

- `config` crate：`Config{ retrieval, upstreams: Vec<UpstreamConfig>, server, audit, dashboard }`；
  `Config::from_toml_str(s) -> Result<Config, ConfigError>`（解析 + 私有 `validate()`，**纯解析、不读 env**）。
  `UpstreamConfig` 派生 `PartialEq`（changed 检测可用 `==`）。
- env 解析器现都在 `mcpgw` bin 的 `main.rs`，启动期 fail-fast：`resolve_api_keys`（http api_key）、
  `resolve_admin_token`（dashboard admin_token_env）、`validate_upstream_http_env`（上游 bearer/header env）、
  `build_backends`（vector/subagent 的 api_key_env）。
- `upstream::connect::connect_all(registry, &[UpstreamConfig], trigger: RebuildTrigger) -> ConnectSummary`：
  逐个连、成功 `registry.insert`、失败 `warn!` + 记 `skipped`（**降级启动**，best-effort）。
  `registry.remove(name) -> Option<Arc<UpstreamHandle>>`（丢弃句柄即断连）。`registry.server_names()`。
- `gateway` 依赖 `upstream`，拥有 `registry` + `rebuild_snapshot()`（M4 已让它读 `DisableSet` 过滤）。
- `main.rs`：`load_config(path)` 读文件 → `Config`（owned）；`run_serve(cfg)` 持有；`cli.config: Option<PathBuf>`。
- dashboard：M4 已有 Bearer 鉴权 admin 子路由（`route_layer(require_admin_token)`）+ `AppState.admin_token`；
  `AppState.gateway: Arc<GatewayState>`。
- 配置文件**无明文密钥**（仓库惯例全 env 引用）。

## 非目标（YAGNI）

- 不引入 `toml_edit`：**全文编辑**（前端提交整份 TOML、后端验证后**逐字写回**），注释/排版是用户自己的文本、
  天然保留；不做结构化字段级编辑。
- 不做 top_k/strategy/server/dashboard 等的热重载——这些需要给 gateway/downstream 新增可变状态
  （`AtomicUsize`/`ArcSwap`），价值次之、复杂度高，留作后续；本期一律"需重启"。
- 不由 dashboard 重启/管理进程（它只是 serve 的子任务）——"需重启"只提示运维手动操作。
- 不做配置版本历史 / 多份备份（单个覆盖式 `.bak`）。
- 不放宽 env 校验：缺任一引用的 env → 拒绝落盘（用户已选"严格"）。

---

## 架构与范围

**新增 2 个端点**（挂在 M4 的 Bearer 鉴权 admin 子路由，端点 18 → 20）：

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/admin/config` | 返回当前 `mcpgw.toml` 原文本（token-gated；文件无明文密钥，原样返回） |
| PUT | `/api/admin/config` | 收完整 TOML 文本 → 严格校验 → 原子落盘(+.bak) → 上游热重载 → 返回 `ApplyResult` |

**两个"放哪"的架构决策**：

1. **校验逻辑（注入）**：严格校验 = 结构 + 所有 env 引用可解析，而 env 解析器在 `main.rs`。dashboard 不该
   反向依赖 main，config crate 须保持纯解析。→ `main.rs` 把
   `config_validator: Arc<dyn Fn(&str) -> Result<Config, String> + Send + Sync>` 注入 `AppState`，内部串起
   `from_toml_str` + 全部 env 解析器；PUT handler 只调它。校验逻辑留在 main、零耦合。

2. **上游热重载（gateway 能力）**：gateway 已依赖 `upstream`、拥有 registry + rebuild。→ gateway 加
   `reconcile_upstreams(old, new, trigger) -> ReconcileSummary`，dashboard handler 调
   `state.gateway.reconcile_upstreams(...)`。复用 `connect_all`/`registry.remove`，best-effort。

**热 vs 需重启**：
- **热应用**：`[[upstream]]` 增/删/改 → connect/remove + rebuild，秒级生效。
- **需重启**（可编辑、可落盘、重启才生效，返回横幅）：`[retrieval]`、`[server]`、`[server.http]`、
  `[dashboard]`、`[audit]` 及各类 api_key/token。

**前提**：M5 需运行时知道配置文件路径。`serve --config X` 时把 `PathBuf` 放进 `AppState`；
未带 `--config`（`default_from_empty`）→ GET/PUT config 返回 **404**（无文件可改）。

**备选（未采纳）**：`toml_edit` 结构化编辑（全文编辑下无谓增依赖/复杂度）；校验下沉到 config crate
（破坏其纯解析、不读 env 的边界）。

---

## 校验与原子持久化

### 注入的校验器（`main.rs` 构造）

`Fn(&str) -> Result<Config, String>`，依次：

```
from_toml_str(text)            → 结构（解析 + validate：必填/范围/deny_unknown_fields）
resolve_api_keys(&cfg)         → [[server.http.api_key]] env（http 启用时）
resolve_admin_token(&cfg)      → [dashboard].admin_token_env（dashboard 启用时）
validate_upstream_http_env(&cfg) → 上游 bearer_env / header env
build_backends(&cfg)           → vector/subagent 的 api_key_env（按 strategy；只解析、构造 client 结构、无 I/O）
```
任一 Err → PUT 返回 **400 + 该错误消息、绝不落盘**。返回 `Ok(Config)` 进入持久化。

### PUT 流程（全程持 `config_write_lock: tokio::Mutex` 串行化）

```
PUT /api/admin/config  (Bearer，经 M4 中间件)
  ├─ config_path == None        → 404（serve 未带 --config）
  ├─ validator(text) == Err(m)  → 400 + m（不写盘）
  └─ Ok(new_cfg):
       ① 原子落盘：写同目录 temp → write_all + flush + fsync
                   → 备份当前文件到 <config>.bak（best-effort，失败仅 warn）
                   → rename temp 覆盖 config（同盘原子，崩溃不留半截）
                   落盘 IO 失败 → 500（原文件/.bak 不动）
       ② 上游热重载：reconcile_upstreams(applied_upstreams, new_cfg.upstreams, trigger)
       ③ needs_restart：new_cfg 的 {retrieval,server,audit,dashboard} 段 vs boot_config（基线）
       ④ 成功 → 更新 applied_upstreams = new_cfg.upstreams.clone()
       └─ 200 Json(ApplyResult{ upstreams: ReconcileSummary, needs_restart: Vec<&str> })
```

### 原子写 + `.bak`

每次保存前把当前文件复制为单个 `<config>.bak`（覆盖式，非时间戳）。顺序 temp+fsync → 备份 → rename 保证任何
时刻 config 要么旧完整、要么新完整。`.bak` 须 gitignore。

### GET `/api/admin/config`（Bearer）

读 `config_path` 当前文本原样返回（`Json{ path: String, content: String }`）。`config_path == None` → 404。

### `AppState` 新增（`main.rs` 装配时注入）

- `config_path: Option<PathBuf>`
- `config_validator: Arc<dyn Fn(&str) -> Result<Config, String> + Send + Sync>`
- `config_write_lock: Arc<tokio::Mutex<()>>`
- `boot_config: Arc<Config>`（启动快照，`needs_restart` 的非上游基线 → "与正在运行的相比仍需重启"，跨多次
  PUT 累计准确）
- `applied_upstreams: Arc<std::sync::Mutex<Vec<UpstreamConfig>>>`（上游 reconcile 的"旧"基线，PUT 成功后更新）
- `rebuild_trigger: RebuildTrigger`（传给 `connect_all`）

`Config` 及其 `retrieval/server/audit/dashboard` 段需派生 `PartialEq`（DashboardConfig 等已派生；缺的补上）。

---

## 上游热重载协调算法

`gateway` 新增（`crates/gateway/src/lib.rs` 或新模块）：

```rust
pub struct ReconcileSummary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub reconnected: Vec<String>,
    pub connect_failures: Vec<(String, String)>, // (name, error)
}

pub async fn reconcile_upstreams(
    &self,
    old: &[UpstreamConfig],
    new: &[UpstreamConfig],
    trigger: RebuildTrigger,
) -> ReconcileSummary;
```

算法（按 `name` 三向 diff）：

```
removed   = old.name − new.name              → registry.remove(name)（丢弃 handle = 断连）
added     = new.name − old.name              ┐
changed   = 同名但 UpstreamConfig != 旧       ┘→ to_connect 子集
                                              → connect_all(registry, &to_connect, trigger)
                                                （逐个连、成功 insert/替换、失败记 skipped）
unchanged = 同名且 config 相等                → 不动连接（零中断）
rebuild_snapshot()  →  新工具集生效；M4 禁用集仍照常过滤（两特性天然组合）
返回 ReconcileSummary{ added, removed, reconnected, connect_failures }
```

- **best-effort**（与 connect_all/启动一致）：某上游连接失败 → 记 `connect_failures`、不中止其它、**不**回滚
  已落盘的新配置（落盘 = 用户意图）；该上游本次缺席，修好后再 PUT 或重启即恢复。
- **changed** 用整体 `UpstreamConfig ==` 判定（任一字段变化即 reconnect；改动罕见，整体 reconnect 最简无歧义）。
- **与 M4 组合**：reconcile 末尾主动 `rebuild_snapshot`；被禁用上游若仍在禁用集 → rebuild 照样跳过（隐藏保持）。

---

## 前端（Config 编辑器，token-gated）

- `lib/admin.svelte.js`：加 `adminGet(path)` / `adminPut(path, body)`（带 `Authorization: Bearer ${admin.token}`，
  返回原始 `Response`）。
- 新 `lib/Config.svelte` + 导航项 **Config**（仅 `admin.token` 非空时显示）：
  - 进入时 `adminGet('/api/admin/config')` → `<textarea>` 载入当前 TOML（含 `refresh` 不强制覆盖正在编辑的内容）；
  - **Save** → `adminPut('/api/admin/config', text)`：
    - 200 → 显示 `ApplyResult`：上游 added/removed/reconnected/**connect_failures** + **需重启**段横幅；
    - 400 → 内联显示校验错误消息；401 → 提示 token 失效；404 → 提示 serve 未带 `--config`。
  - 复用现有 `.error`/`.badge`/`.admbtn` 风格；`<textarea>` 等宽字体；**无 `{@html}`**（XSS guard）。

---

## 数据流

```
前端编辑 ──Bearer──▶ PUT /api/admin/config ──(config_write_lock)──▶ validator(text)
                                                                  ├─ Err → 400（不写盘）
                                                                  └─ Ok(new_cfg):
   原子写(.bak) ──▶ reconcile_upstreams(applied,new,trigger) ──▶ connect/remove + rebuild_snapshot
                 └─▶ needs_restart = diff(new_cfg, boot_config 非上游段)
                 └─▶ 更新 applied_upstreams ──▶ 200 ApplyResult
```

---

## 测试策略（Rust 为主；重 corner + 核心；前端仅构建冒烟）

- **校验器**（main.rs，复用 resolve_*）：合法 → Ok；坏 TOML / 非法值 / 未知字段 → Err；各类 env 未设
  （api_key / admin_token_env / bearer_env / vector·subagent api_key_env）→ Err。
- **原子写**：写新内容 + 生成 `.bak`(旧内容)、无残留 temp、内容一致；不可写路径 → 报错 + 原文件不动。
- **`reconcile_upstreams`**（gateway + MockUpstream）：add → 新工具现身；remove → 消失 + registry 移除；
  change → reconnect 反映；unchanged → 连接不动；connect 失败 → 记 `connect_failures`、不中止、不回滚；
  **与 M4 组合**：热加一个在禁用集里的上游仍隐藏。
- **PUT/GET handler**：config_path=None → 404；校验失败 → 400 不写盘；合法 → 200 + 落盘 + 热应用 +
  `applied_upstreams` 更新；非上游段变更 → `needs_restart`（vs boot_config）；写锁串行。
- **鉴权**：把 M4 的参数化边界测试扩到 `/api/admin/config` 的 GET/PUT（未配 → 404、无/错 Bearer → 401、
  开放读端点仍 200）。
- **构建**：`npm run build` 复现 dist；新组件编译。

## 文档（L1–L4，随代码同提交）

- L1-overview：写子系统 C（在线改配 + 上游热重载）；端点 18 → 20。
- L2/L3 dashboard：在线改配子系统——严格校验（结构 + 全 env）/ 原子写 + .bak / reconcile / 需重启语义 /
  不重启进程；端点 18 → 20。
- L2/L3 gateway：`reconcile_upstreams` 三向 diff + best-effort + 与 M4 组合。
- L4 dashboard：`GET/PUT /api/admin/config` + handler；L4 mcpgw-main：`config_path` + `config_validator` /
  `boot_config` / `applied_upstreams` / `rebuild_trigger` 注入。
- L2/L3 config：env 校验复用说明（仍在 main、经注入）。
- `.gitignore`：加 `*.bak`。

## 交付流程（仓库惯例）

subagent-driven-development，**每 task 跑完整 spec + 质量双重审查**、折叠 nit、整分支 `code-review`、`--no-ff`
本地合并、复测、删分支、推 origin；门禁 `cargo fmt --all --check` + `cargo clippy --all-targets --all-features
-D warnings` + `cargo test --all-features` + `cargo build --locked` + `npm run build` 复现 `dist`。
分层文档 L1–L4 同步、L1 测试计数更新。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| 落盘后热应用部分失败，配置与运行态不一致 | best-effort + `ApplyResult` 如实报告 connect_failures / needs_restart；落盘是用户意图不回滚；修好再 PUT 或重启 |
| 原子写崩溃损坏配置 | temp→fsync→备份 .bak→rename，任何时刻 config 完整；`.bak` 兜底 |
| 严格校验挡住"先改配后设 env"工作流 | 已与用户确认采纳（更安全）；错误消息明确指出缺哪个 env |
| 校验/写/reconcile 并发 | `config_write_lock` 串行整个 PUT；reconcile 末尾单次 rebuild（rebuild_lock 既有串行） |
| 写凭据经非 loopback 明文 HTTP | 与 M4 同：admin 假定 loopback/TLS 前置；`unauthenticated_public_bind` 已 warn；文档注记 |
| 依赖方向 | 校验器注入（逻辑留 main）；reconcile 落 gateway（已依赖 upstream）；均不引入 dashboard→main 依赖 |
