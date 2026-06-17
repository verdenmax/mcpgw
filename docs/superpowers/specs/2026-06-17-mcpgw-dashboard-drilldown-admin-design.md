# mcpgw 可视化面板下钻导航 + 在线管控设计

日期：2026-06-17
状态：已批准设计（待写实施计划）

## 背景与目标

现有 dashboard（子系统 A 只读面板，已合并）是一个 55 行 vanilla JS 的**单页扁平面板**：一个 `refresh()` 轮询 5 个
`/api/*` 端点、把 4 个面板整体重渲染，没有路由、没有「点进去看详情」的能力，后端也**只存聚合数**（`MetricsSink`
的 per-meta / per-upstream 计数与直方图）+ 搜索追踪环（`DiscoveryRingSink`），**没有逐条调用记录**。

本设计把它演进成一个**多页、可下钻的运维控制台**，并在只读浏览之上叠加**运行时管控**能力：

1. **下钻导航（读）**：导航分 Overview / Upstreams / Tools / Calls / Traces；每个区域都能从「概览数字」点进「列表」、再点进「单条详情」，层层深入。例如点 Calls 里 `call_tool` 的指标卡 → 进入该 meta-tool 的**逐条调用列表** → 点某条 → **单次调用详情**（元数据）。
2. **运行时临时禁用（写）**：在页面上临时禁用某个 upstream 或 tool（隐藏式：从快照过滤 + 路由拒绝，保留底层连接，秒级恢复；纯内存、重启复位）。
3. **在线改配置 + 热重载（写）**：在页面编辑 `mcpgw.toml` 全部字段并持久化；其中 upstreams 增删改与检索参数可**真正热重载**，其余字段可编辑+落盘但**需重启生效**。

**安全立场**：读保持现状（仅 localhost + 反 DNS-rebinding 的 Host 检查，不鉴权）；**所有写操作要求 `Authorization: Bearer <token>`**，token 配在 `[dashboard].admin_token` / `admin_token_env`，**未配则写接口全部禁用（默认安全）**。

### 非目标（本设计明确不做）

- 不引入用户系统 / 多账号 / RBAC：写鉴权就是单一共享 admin token。
- 不做 WebSocket / SSE 实时推送：前端继续轮询 `/api/*`（与现状一致）。
- 不在 `CallRecord` 里加入参数/结果内容：逐条调用**只展示元数据**（隐私洁净保持不变）。
- 不支持热重载监听端口 / 传输开关 / 鉴权 token / audit 设置（架构上启动期绑定，归入「需重启」类）。

## 范围与里程碑拆分

这是 3 个风险递增的子系统，拆 5 个**各自可独立交付**的里程碑，逐个 spec→plan→实现：

| 里程碑 | 内容 | 子系统 | 依赖 |
|---|---|---|---|
| **M1 逐条调用数据层** | `CallRingSink` 内存环 + audit JSONL 历史回放 + 列表/详情只读 API | A | 无 |
| **M2 前端应用骨架** | Svelte+Vite 构建链 + rust-embed + hash 路由 + Overview/Calls 下钻视图 | A | M1 |
| **M3 Upstreams/Tools/Traces 下钻** | 三区域详情页 + 交叉链接 | A | M2 |
| **M4 运行时临时禁用** | 禁用集 + 快照过滤/路由拒绝 + 写 API + Bearer 鉴权中间件 | B | M2 |
| **M5 在线改配 + 热重载** | 配置读/校验/落盘（toml_edit）+ 可重载字段热应用 + 写 API | C | M4（复用鉴权） |

每个里程碑结束都能 ship 并演示。M1+M2 即交付「Calls 下钻」核心价值；M3 补齐其余区域；M4/M5 叠加写能力。

## 架构总览

进程模型不变：`mcpgw serve` 一个进程，dashboard 仍是独立端口、独立 tokio 任务、panic 边界、`with_graceful_shutdown`。本设计的新增物：

```
crates/dashboard/
├─ src/
│  ├─ lib.rs        路由装配 + Host 检查（已有）+ 新增写鉴权中间件层
│  ├─ api.rs        只读视图函数（已有）+ 新增 calls/详情/单实体视图
│  ├─ calls.rs      新增：CallRingSink（逐条环）+ 过滤/分页查询
│  ├─ history.rs    已有 replay_discovery/replay_audit_metrics + 新增 replay_audit_calls
│  ├─ metrics.rs    已有聚合 MetricsSink（不动）
│  ├─ trace.rs      已有 DiscoveryRingSink（不动）
│  ├─ admin.rs      新增（M4/M5）：禁用集句柄、写 API handler、token 校验
│  └─ assets.rs     新增（M2）：rust-embed 嵌入 ui/dist/
└─ ui/              新增（M2）：Svelte+Vite 工程
   ├─ src/          组件、路由、视图
   ├─ dist/         构建产物（入库，cargo 不依赖 node）
   ├─ package.json / vite.config.ts
```

依赖方向保持 `dashboard → {gateway, observe, catalog, config}`；新增 crate 依赖：`rust-embed`（M2）、`toml_edit`（M5）。核心 crate 不反向依赖 dashboard。

## M1：逐条调用数据层

**核心缺口**：`MetricsSink` 只聚合，要支持「列出那 4 次 call_tool」必须有逐条记录源。

### CallRingSink（新增，`crates/dashboard/src/calls.rs`）
- 实现已有 `observe::CallSink` 接缝，加入 main.rs 的 sink 扇出（与 `MetricsSink` 并列，仅 `[dashboard].enabled` 时挂载）。
- 内部 `Mutex<VecDeque<CallRecord>>`，容量上界 `[dashboard].call_buffer`（默认 2000）；满则 `pop_front`（丢最旧），内存天然有界。锁不跨 `.await`（`CallRecord` 已是元数据值类型，clone 廉价）。
- `CallRecord` 字段沿用现状（`ts_unix_ms / meta_tool / target_tool / upstream / latency_ms / outcome / error_kind / arg_bytes / result_bytes`），**不新增内容字段**。
- 查询方法：`fn query(&self, filter: &CallFilter, limit, offset) -> (Vec<CallRecord>, total)`，过滤维度：`meta_tool`、`upstream`、`target_tool`、`outcome`、`since_ms/until_ms`。返回**最新优先**。

### audit JSONL 历史回放（扩展 `history.rs`）
- 新增 `replay_audit_calls(path, limit, filter) -> (Vec<CallRecord>, bool)`，复用既有 `tail_lines`（有界尾读、最新优先、坏行跳过），把 audit 行反序列化为 `CallRecord` 再按 `CallFilter` 过滤。
- 仅当 `[audit].enabled` 时该数据源可用；与 Traces 页「实时环 + 可选历史回放」完全一致的双源模型。

### 只读 API（M1 暴露，M2 消费）
- `GET /api/calls?meta=&upstream=&tool=&outcome=&source=live|history&limit=&offset=` → `{ items: CallRecord[], total, source }`
- `GET /api/calls/:id` → 单条详情。**id 定义**：`CallRingSink` 在每次 insert 时分配一个进程内单调递增的 `u64` seq，作为 live 记录的稳定 id（环淘汰后该 id 失效，返回 404）；history 记录用 `ts_unix_ms` 在该毫秒内的行内偏移组成 `"<ts>-<n>"` 复合 id。详情即该 `CallRecord` 全字段 + 交叉链接到其 upstream/tool。
- 沿用 `api.rs` 纯函数 + handler 的现有风格；查询参数有界（limit 夹紧上限，复用 N2 风格的 clamp）。

### M1 测试
- CallRingSink：满环淘汰最旧、过滤各维度、分页、并发写不丢锁不跨 await。
- replay_audit_calls：缺文件不可用、坏行跳过、最新优先、过滤生效（镜像 history.rs 既有测试）。
- corner：空环、limit=0、offset 越界、since>until。

## M2：前端应用骨架（Svelte + Vite + rust-embed）

### 构建链
- `crates/dashboard/ui/`：Svelte + Vite 工程，`npm run build` → `ui/dist/`（带 hash 的多文件 JS/CSS）。
- **产物入库**：`ui/dist/` commit 进仓库；`npm run build` 只是 UI 开发者的再生步骤，`cargo build --locked` **不需要 node**、可复现、CI 不被破坏。
- **rust-embed 内嵌**：`assets.rs` 用 `#[derive(RustEmbed)]` 嵌入 `ui/dist/`，保持「单一自包含二进制」部署。路由 fallback 把未知路径回退到 `index.html`（支持前端 hash 路由的深链接刷新）。
- 移除旧的三文件 `include_str!`（index.html/app.js/style.css）与对应 `/app.js`、`/style.css` 路由。

### 前端结构
- **hash 路由**（`#/overview`、`#/calls`、`#/calls/call_tool`、`#/calls/:id`、`#/upstreams/:name`、`#/tools/:name`、`#/traces/:id`）。hash 路由 + rust-embed fallback 即可深链接刷新，无需服务端路由表。
- **布局**：左侧固定导航（Overview/Upstreams/Tools/Calls/Traces）+ 右侧主区（已在批准的 `nav-shell.html` mockup 中定型）。
- **视图组件**：每区域一个「列表视图」+「详情视图」；列表支持过滤 chips + 点行下钻；详情展示全字段 + 交叉链接（点 upstream 名跳 `#/upstreams/:name`，点 tool 名跳 `#/tools/:name`）。
- **数据层**：一个轻量 `api.ts` 封装 `fetch /api/*`；轮询保持（与现状一致），可见页才轮询。

### M2 交付范围
本里程碑只接通 **Overview + Calls** 两区域的下钻（消费 M1 的 `/api/calls`），其余区域（Upstreams/Tools/Traces 详情）留给 M3。导航项可见但未接通的先占位。

### M2 测试
- Rust 侧：rust-embed 资产存在性（index.html 含挂载点）、未知路径回退 index.html、`/api/*` 仍可用。
- 构建产物存在性断言（`ui/dist/index.html` 在仓库内）。
- 前端：Vite 构建通过即可（不引入前端测试框架，保持依赖最小；逻辑薄，靠后端 API 测试 + e2e 冒烟覆盖）。

## M3：Upstreams / Tools / Traces 下钻

在 M2 骨架上补齐其余三区域的列表→详情：
- **Upstreams 详情**（`#/upstreams/:name`）：该上游的传输类型、连接状态、工具数、调用数/错误率（来自 `MetricsSink` per-upstream），其工具列表（交叉链接 Tools），最近调用（过滤 `/api/calls?upstream=`）。新增 `GET /api/upstreams/:name`。
- **Tools 详情**（`#/tools/:name`）：所属 upstream、描述、input schema，最近调用（`/api/calls?tool=`）。新增 `GET /api/tools/:name`（复用 catalog `get(qualified_name)`）。
- **Traces 详情**（`#/traces/:id`）：单条搜索追踪的 query、top_k、命中工具+分数、延迟（来自 `DiscoveryRingSink`/历史回放）。新增 `GET /api/traces/:id`。
- Overview 卡片全部接成可点击的交叉链接。

### M3 测试
- 各 `/api/{upstreams,tools,traces}/:id` handler：命中、未命中 404、交叉过滤正确。
- corner：不存在的 name、特殊字符 name 的 URL 编码。

## M4：运行时临时禁用（写子系统 #1）

### 禁用集
- `GatewayState` 新增 `disabled: Arc<DisableSet>`（内部 `RwLock<{ upstreams: HashSet<String>, tools: HashSet<String> }>`），cheaply-cloneable，跨重建保持，纯内存。
- **隐藏式语义**：
  1. **发现层过滤**：`rebuild_snapshot` 构建 catalog 时跳过被禁用 upstream 的全部工具与被禁用的单个工具 → 搜索/discovery 看不到。禁用动作触发一次 `rebuild_snapshot`。
  2. **路由层拒绝**：`call_tool` / `get_tool_details` 命中被禁用目标时返回 `MetaError`（映射为 isError），即便快照尚未重建也兜底拒绝（防竞态）。
  3. **保留连接**：不动 `UpstreamRegistry` 句柄，重新启用只需从禁用集移除 + 触发重建，秒级恢复。

### 写 API（需 Bearer token）
- `POST /api/admin/upstreams/:name/disable` / `.../enable`
- `POST /api/admin/tools/:name/disable` / `.../enable`
- `GET  /api/admin/disabled` → 当前禁用集（也供前端渲染禁用状态徽标）
- 前端：Upstreams/Tools 列表与详情页加「禁用/启用」开关（仅在已配 token 且用户填入 token 时可用；token 存浏览器内存，不落 localStorage）。

### 鉴权中间件
- `admin.rs` 新增 axum 中间件 `require_admin_token`，仅挂在 `/api/admin/*` 子路由上：校验 `Authorization: Bearer <token>` 常量时间比较；未配 token → 整个 `/api/admin/*` 不挂载（返回 404，等价禁用）；配了但不匹配 → 401。
- token 解析复用现有 env 引用风格（`admin_token` 直配 或 `admin_token_env` 指向环境变量，启动期 fail-fast 解析，与 `[[server.http.api_key]]` 一致）。

### M4 测试
- DisableSet：禁用后快照过滤、路由拒绝、启用后恢复、并发开关。
- 中间件：无 token 配置→404、错 token→401、对 token→放行、常量时间比较。
- 端到端：禁用 upstream 后 search_tools 不再返回其工具、call_tool 被拒。

## M5：在线改配 + 热重载（写子系统 #2）

### 配置读/校验/落盘
- `GET /api/admin/config`（需 token）→ 当前 `mcpgw.toml` 文本 + 解析后的结构化视图。**secret 脱敏**：env 引用字段只回显引用名不回显值。
- `PUT /api/admin/config`（需 token）→ 收新 TOML 文本：
  1. **解析校验**：`toml_edit` 解析 + 复用现有 `config` 校验（结构、必填、env 引用可解析）。
  2. **原子落盘**：写临时文件 → `fsync` → `rename` 覆盖 `mcpgw.toml`；覆盖前留 `.bak` 备份。
  3. **格式保留**：`toml_edit` 保留注释与排版，只改动到的字段。
  4. 返回字段级 diff + 每个改动的「热重载 / 需重启」分类标注。

### 字段分类
- **A 可热重载**（改了立即生效，保存后触发对应动作）：
  - `[[upstreams]]` 增/删/改 → `connect_all`（新增/变更的）+ `registry.remove`（删除的）+ `rebuild_snapshot`。
  - `[retrieval] top_k` / `strategy` → 需把这两个值从「启动期捕获」改为「`GatewayState` 内可换」（`top_k` 用 `AtomicUsize`，`strategy_name` 用 `ArcSwap<str>`），改后触发 `rebuild_snapshot`。
- **B 可编辑可落盘、需重启生效**：`[server.http] bind/path/enabled`、`[server] stdio`、`[dashboard] bind/enabled`、`[server.http].api_key`、`[dashboard].admin_token`、`[audit] enabled/path`。UI 保存后对这些字段显示「需重启生效」横幅。

### 热重载执行
- `admin.rs` 比较新旧 config：仅对 A 类字段执行热应用；任一 upstream 连接失败不回滚已落盘配置（落盘是用户意图），但在响应里报告哪些 upstream 连接失败（skipped），与 `RebuildSummary` 一致的「尽力而为」语义。
- 写操作串行化（`admin.rs` 内一把 `tokio::Mutex`）防并发改配竞态。

### M5 测试
- 配置往返：解析→`toml_edit` 改字段→落盘→重读一致；注释保留；原子写（中断不损坏原文件）；`.bak` 生成。
- 校验：坏 TOML 拒绝、无效 env 引用拒绝、缺必填拒绝——均不落盘。
- 热重载：加 upstream→快照出现其工具；删 upstream→消失 + registry 移除；改 top_k→后续 search 生效。
- 分类：B 类字段改动返回「需重启」标注、不尝试热应用。
- secret 脱敏：env 引用值不出现在 `GET /api/admin/config` 响应里。

## 数据流

**读下钻（M1-M3）**：
```
CallRecord ──fan-out──▶ CallRingSink(环) ──┐
                                            ├─▶ GET /api/calls(?filter) ──▶ Svelte 列表 ──点行──▶ GET /api/calls/:id ──▶ 详情
audit.jsonl ──replay_audit_calls──────────┘   (live | history 双源)
```

**写禁用（M4）**：
```
前端开关 ──Bearer──▶ POST /api/admin/.../disable ──▶ DisableSet.insert ──▶ rebuild_snapshot(过滤) ──▶ ArcSwap 原子换 ──▶ 搜索/路由即时隐藏
```

**写改配（M5）**：
```
前端编辑 ──Bearer──▶ PUT /api/admin/config ──▶ toml_edit 校验 ──▶ 原子落盘(.bak) ──▶ diff 分类
                                                                       ├─ A类 ──▶ connect/remove + rebuild_snapshot(热生效)
                                                                       └─ B类 ──▶ 仅落盘 + 「需重启」标注
```

## 错误处理

- **数据层**：环为空 / 文件缺失 → 返回空集 + `source` 可用性布尔（不报错），与现有 traces/history 一致。
- **鉴权**：未配 token → `/api/admin/*` 整体 404（不泄露存在性）；配了不匹配 → 401；常量时间比较防计时攻击。
- **改配**：任何校验失败都在落盘前拒绝并回错误详情；落盘用临时文件+rename 保证原子（崩溃不留半截文件）；热应用失败只报告不回滚已落盘文本。
- **隔离**：所有新缓冲有界（满则丢最旧）；dashboard 任务 panic 边界不变；写操作串行化。

## 测试策略

- 沿用现有门禁：`cargo fmt --all --check`（硬门）、`cargo clippy --all-targets --all-features -D warnings`、`cargo test --all-features`、`cargo build --locked`。
- 每个里程碑：单元（环/回放/禁用集/校验/落盘）+ handler + 端到端冒烟（沿用现有 `#[ignore]` e2e 模式）。
- 重点补 corner（用户偏好）：满环淘汰、limit/offset 边界、并发开关、原子写中断、坏 TOML / 坏 env 引用、未配 token、URL 编码特殊 name。
- 前端不引入测试框架（保持依赖最小），逻辑下沉到可测的后端 API + e2e。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| 在线改配含 stdio `command` = RCE | 写操作强制 Bearer token；未配则写接口不存在；token 常量时间比较；读不暴露写 |
| 构建产物入库 = 二进制 diff 噪声 | `ui/dist/` 体积小（Svelte 产物最小）；CI 可加「dist 与 src 同步」校验（可选，非本期硬性） |
| 热重载部分字段不可热换造成误解 | 字段显式 A/B 分类 + UI「需重启」横幅，不假装能热换 |
| 逐条环重启丢失 | audit JSONL 历史回放兜底（需 `[audit].enabled`） |
| 新增 rust-embed/toml_edit 依赖 | 两者都是成熟轻量库；仅 dashboard crate 引入，不污染核心 |

## 文档

每个里程碑落地时同步分层文档（用户偏好 L1-L4，与代码一起提交）：
- L1 概览：补「下钻导航 + 在线管控」段落、测试计数。
- L3 细节：dashboard 的数据层（CallRingSink）、鉴权模型、热重载分类。
- L4 API：新增端点（`/api/calls*`、`/api/admin/*`）逐一记录；`calls.rs`/`admin.rs`/`assets.rs` 逐文件 API。
