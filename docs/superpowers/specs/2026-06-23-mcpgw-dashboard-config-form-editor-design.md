# mcpgw Dashboard 配置编辑器：结构化表单模式（写子系统 C 增强）

- 日期：2026-06-23
- 状态：设计已批准，待实施
- 关联：在 M5（`2026-06-23-mcpgw-dashboard-config-edit-hot-reload-design.md`）交付的 raw 全文编辑之上做增强

## 1. 背景与动机

M5 交付了 raw 全文 TOML 编辑（`GET/PUT /api/admin/config`）：前端一个 `<textarea>`，后端接收整段 TOML 文本，做严格校验（结构 + 全 env 引用可解析）→ 原子写(.bak) → `[[upstream]]` 热重载。

raw 模式**保真**（保留用户的注释与排版）、实现简单，但代价是：

- 没有字段级约束、没有枚举提示，全靠用户记住 schema；
- 写错（拼写、类型、枚举值越界）只有在 Save 时由后端兜底报错，反馈链路长。

本设计在其上增加一个**结构化表单模式**，与 raw 模式并存，提供"现代化、有限制的输入框 + 枚举下拉"的引导式编辑，降低出错、提升可发现性。

## 2. 目标 / 非目标

**目标**

- 覆盖**全部配置段**（`retrieval` / `server` / `audit` / `dashboard` / `[[upstream]]`）的结构化表单。
- `Raw │ Form` 两视图**实时双向同步**，共享同一份纯数据 model。
- 字段级**即时校验** + **枚举下拉** + 必填/类型约束。
- 复用 M5 现有 `PUT /api/admin/config` 写路径（严格校验 + 原子写 + 上游热重载），**后端零改动**。

**非目标**

- 表单模式**不保留注释/格式**：保存即规范化；需要保真请用 raw 模式，且仅在"全程不切到表单"时成立（见 §3 取舍）。
- 不做 schema 端点驱动的通用渲染器（采用前端硬编码表单）。
- 不改后端 API / 数据契约 / 校验逻辑。

## 3. 关键设计决策（含取舍）

| 维度 | 选择 | 理由 / 取舍 |
| --- | --- | --- |
| 覆盖范围 | **全段** | 用户要图形化整个配置；代价是前端镜像整个 schema（见 §10 漂移缓解） |
| 保真 | **表单保存即规范化（丢注释）**；raw 仅"纯 raw 工作流"保真 | 全段表单 + 保留注释需 toml_edit 级 AST，复杂度过高；规范化是现实选择 |
| 两视图同步 | **实时双向、纯数据 model** | 单一数据源、切换即时；与"raw 保真"冲突已明确接受（一旦切表单即规范化） |
| 架构 | **前端 TOML 库 + 硬编码全段表单 + 复用 PUT、后端零改动** | 最契合"实时本地同步"，复用已验证的校验/热重载路径，表单 UX 可精细 |
| 布局 | **A · 左段导航 + 右表单**（VS Code 设置风格） | 段较多时定位最快，右侧空间足够放 upstream 数组与 stdio/http 差异字段 |
| 前端测试 | **引入 vitest** | round-trip 与字段校验是核心风险点，需单测兜底（契合"多测试"偏好） |

## 4. 架构与数据流

后端不动，全部增量在 `crates/dashboard/ui`。核心是 `raw 文本` 与 `model` 两个视图共享、由前端 TOML 库双向转换：

```
后端 GET /api/admin/config ──content(toml)──▶ ┌──────────┐
                                              │ raw 文本  │◀─┐
                                              └────┬─────┘  │
                                        parse│      ▲stringify
                                             ▼      │        │
                                          ┌──────────────┐   │ 切到 Raw 时
                                          │  表单 model   │   │ 由 model 反向
                                          │ {retrieval,  │   │ 生成 raw
                                          │  server,     │───┘
                                          │  audit,      │
                                          │  dashboard,  │
                                          │  upstreams[]}│
                                          └──────────────┘
                                                  ▲ 表单各段绑定 model 字段
 Save：当前视图 → stringify(model)→toml → 现有 PUT /api/admin/config
        → 后端严格校验 / 原子写(.bak) / 上游热重载
        → 结果卡片（upstreams +/−/~、connect_failures、needs_restart）
```

**同步语义**

- 切到 **Form**：`parse(raw) → model`；若 raw 是非法 TOML → 解析失败 → 禁用表单并提示"raw 有语法错误，修正后可结构化编辑"。
- 切到 **Raw**：`stringify(model) → raw`（规范化输出，丢注释）。
- 编辑哪个视图就更新哪个的数据源，切换时同步另一侧。
- 一旦经 Form 编辑并切回 Raw（或 Save），输出即规范化 TOML——符合 §3 取舍。

**前端 TOML 库**：封装为 `lib/toml.js`（`parse` / `stringify`），推荐 **smol-toml**（现代 ESM、支持双向、体积小）；最终选型在实施计划阶段确认（须验证 `[[array-of-tables]]`、嵌套表、类型保真、stringify 正确性）。

## 5. 组件分解

| 组件 | 职责 | 依赖 |
| --- | --- | --- |
| `Config.svelte`（容器，改造现有） | 顶部 `Raw │ Form` 切换 + `Save`/`Reload` + 错误/结果卡片；持有单一 `model` 与 `raw`，负责两视图同步与 Save | `admin.svelte.js`、`toml.js`、`RawEditor`、`FormEditor` |
| `RawEditor.svelte`（抽出现有 textarea） | raw TOML 文本编辑，行为与现状一致 | — |
| `FormEditor.svelte` | 布局 A：左段导航 + 右侧渲染当前段；持有"当前段"状态 | 各 `Section*` |
| `SectionRetrieval/Server/Audit/Dashboard.svelte` | 各自段的字段表单（含子表、枚举、开关、number） | 绑定 `model.<section>` |
| `SectionUpstreams.svelte` | upstream **数组**：条目增删 + 每条 `transport` 枚举切换（stdio/http 字段集互斥） | 绑定 `model.upstreams[]` |
| `lib/toml.js` | `parse(raw)→model` / `stringify(model)→raw`；解析错误返回结构化信息 | smol-toml（或同类） |
| `lib/configSchema.js`（可选） | 字段元数据（枚举值、必填、热/重启标注）集中管理，供各 Section 与校验复用 | — |
| `lib/validate.js` | 纯函数字段级校验（必填/枚举/number 越界/name 规则/格式） | `configSchema.js` |
| `admin.svelte.js`、后端 `admin_config.rs` | **不变** | — |

设计原则：每个 `Section*` 只认 `model` 的对应子树、职责单一、可独立理解与测试；`toml.js` 与 `validate.js` 是纯函数，便于 vitest 覆盖。

## 6. 字段 Schema 与约束

控件约定：`select`=枚举下拉、`switch`=布尔开关、`number`=数值输入（带 min）、`text`=文本、`env名`=只填环境变量名（绝不填密钥值）、`list`=字符串列表增删、`kv`=键值表。`⟳`=改动需重启生效、`🔥`=改动热生效。

**`[retrieval]` `⟳`**

| 字段 | 控件 | 约束 |
| --- | --- | --- |
| `strategy` | select `[bm25 \| vector \| subagent]` | 必填，默认 `bm25` |
| `top_k` | number | ≥1，默认 10 |
| `vector.*`（`strategy=vector` 时展开） | — | `base_url` text(默认值) · `model` text 必填 · `api_key_env` env名 必填 · `dim`/`timeout_ms`/`batch_size` number 可选 |
| `subagent.*`（`strategy=subagent` 时展开） | — | `base_url` text(默认值) · `model` text 必填 · `api_key_env` env名 必填 · `timeout_ms`/`candidates` number 可选 |

**`[server]` `⟳`**

| 字段 | 控件 | 约束 |
| --- | --- | --- |
| `stdio` | switch | bool |
| `http`（可选块） | — | `enabled` switch · `bind` text · `path` text · `api_key[]` 数组：`name` text(标签) + `env` env名 |

**`[audit]` `⟳`**：`enabled` switch · `path` text。

**`[dashboard]` `⟳`**

| 字段 | 控件 | 约束 |
| --- | --- | --- |
| `enabled` / `trace_queries` | switch | bool |
| `bind` | text | host:port |
| `trace_buffer` / `call_buffer` / `payload_max_bytes` | number | ≥0 |
| `trace_path` / `admin_token_env` / `disabled_state_path` | text / env名 / text | 可选 |

**`[[upstream]]` `🔥`（数组，条目增删）**

| 字段 | 控件 | 约束 |
| --- | --- | --- |
| `name` | text | 必填、非空、不含 `__`、数组内唯一 |
| `call_timeout_ms` | number | ≥1，默认 30000 |
| `transport` | select `[stdio \| http]` | 必填；切换时 stdio/http 字段集互斥 |
| stdio：`command` | text | 必填 |
| stdio：`args` / `env_passthrough` | list | 字符串列表 |
| http：`url` | text | 必填 |
| http：`bearer_env` | env名 | 可选 |
| http：`headers` | kv | header名 → env名 |

## 7. 校验与错误处理

**两层校验**

1. **前端字段级即时**（`validate.js` 纯函数，内联红框/提示，挡明显错误）：必填空、number 非法/越界、枚举越界（下拉天然约束）、`name` 含 `__` 或数组内重复、`bind`/`url` 轻量格式检查。存在字段级错误时禁用 `Save`。
2. **后端权威**（Save 时复用现有 `config_validator`）：结构完整性 + **所有 env 引用可解析**（`api_key_env` / `admin_token_env` / `bearer_env` / `headers` 指向的环境变量是否存在）——前端无法判断，返回 400 + 消息显示在结果区。

**env 字段语义**：只输入**环境变量名**（如 `MCPGW_DASH_ADMIN`），绝不输入/显示密钥值（与现有一致）。

**错误/结果处理**

- raw 非法 TOML → 切 Form 禁用表单 + 提示；
- 后端 401（token 失效，引导去 About）/ 404（未带 `--config`）/ 400（校验失败显示消息）复用现有提示；
- Save 成功 → 复用现有结果卡片（upstreams `+/−/~`、`connect_failures`、`needs_restart`）。

## 8. 测试策略

**前端（新增 vitest，`"test": "vitest run"`）**

- `toml.test.js` **round-trip**：代表性 config（bm25/vector/subagent + stdio/http upstream + http `api_key[]` + 各可选字段）→ `parse→model→stringify→re-parse`，断言语义等价（值不丢不变、类型保真）。
- `validate.test.js` 字段级校验 corner cases：必填空、枚举越界、number 越界、`name` 含 `__`/重复、`bind`/`url` 格式。

**后端**：零改动 → 无新测试（现有 `admin_config` PUT 测试已覆盖写/校验/热重载）。可选加 1 个 Rust 契约测试：用与前端相同的"规范化 TOML 样例" fixture 跑 `config::Config::from_toml_str`，双向锚定 schema、降低漂移。

**手动（demo）**：Raw↔Form 切换保真、各段编辑、upstream 增删与 transport 切换、Save 触发热重载 + needs_restart 呈现、raw 语法错误时表单禁用。

## 9. 验收标准 / Gates

- 后端 `cargo fmt --all --check` / `clippy --all-targets --all-features -D warnings` / `test --all-features` / `build --locked` 全绿（后端零改动，不应受影响）。
- 前端 `npm run test`（vitest）绿。
- `npm ci && npm run build` 重新生成 committed `crates/dashboard/ui/dist/` 且**字节级可复现**（新增 TOML 库 + 组件后，`package-lock.json` 锁定、dist 重新提交）。
- demo 端到端手测通过（§8）。

## 10. 风险与缓解

- **前端 schema 镜像漂移**：config 各段 `#[serde(deny_unknown_fields)]`，前端 stringify 出的 TOML 不能含未知键、且必须覆盖必填键，否则后端 400。缓解：round-trip 测试 + 后端契约 fixture + 文档约定"改 config schema 时同步前端表单/schema 元数据"。
- **TOML 库选型/正确性**：必须正确处理 `[[array-of-tables]]`、嵌套表、整数/字符串类型、stringify。实施计划阶段先用 round-trip 测试验证 smol-toml（或替代）后再定。
- **bundle 体积**：选小型 ESM 库；dist 体积小幅增加可接受（committed dist 仍可复现）。
- **upstream 未知键**：`UpstreamConfig` 因 `#[serde(flatten)]` 不 `deny_unknown_fields`、未知键被静默忽略；硬编码表单天然不产生未知键，反而比手写 raw 更安全。

## 11. 实施范围边界与文档

- **代码**：仅 `crates/dashboard/ui`（新增组件 + `toml.js`/`validate.js`/`configSchema.js` + vitest + `package.json`/`package-lock.json` + 重建 `dist/`）。后端 `crates/dashboard/src/*` 与 API **不动**。
- **文档**：按 L1–L4 同步——主要更新 dashboard 组件相关层（新增"结构化表单模式"：两视图同步、字段约束、前端 TOML 库、vitest）；后端 API 文档不变（契约未变）。
