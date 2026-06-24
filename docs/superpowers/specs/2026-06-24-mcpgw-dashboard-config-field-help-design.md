# mcpgw Dashboard Config 字段说明（inline + tooltip 混用）

- 日期：2026-06-24
- 状态：设计已批准，待实施
- 关联：在 Config 结构化表单（`2026-06-23-mcpgw-dashboard-config-form-editor-design.md`）+ 视觉重设计之上，加**字段级说明**

## 1. 背景与动机

Config 表单字段名直接来自 TOML key（`strategy`/`top_k`/`api_key_env`/`bearer_env`…），不查文档难知其义与取值。为每个字段加一句说明，降低理解成本。已用 retrieval 段 mock 验证了观感与两种形式。

## 2. 目标 / 非目标

**目标**：为全部 5 段（retrieval/server/audit/dashboard/upstream）的每个字段加说明，**混用**两种形式（按下方原则 A 划分）：
- **inline 小灰字**（`.cfg-hint`）：必填/语义关键/容易选错的字段——说明常驻、一眼可见。
- **`?` hover tooltip**（`.cfg-q` + `title`）：可选/有默认/技术细节字段——说明收起、省空间。

**非目标（严格不变）**：校验/同步/Save/`pruneModel`/`validateModel`/后端/数据流。纯文案 + 视觉添加；现有 28 vitest 必须保持全绿。

## 3. 设计

### 3.1 划分原则 A
必填 / 语义关键 / 容易选错 → **inline**；可选 / 有默认值 / 纯技术细节 → **tooltip**。

### 3.2 文案来源
**内联**写在各 `Section*.svelte` 的字段旁（如 mock），不引入集中 map（YAGNI：文案就近、改字段时同处可改、零间接层、逻辑不动）。

### 3.3 形式与样式（mock 已验证，正式化到 `app.css`）
```css
.cfg-hint { font-size: var(--fs-2xs); color: var(--muted); line-height: 1.45; }
.cfg-q { display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px;
  border-radius: 50%; background: var(--panel); border: 1px solid var(--border); color: var(--muted);
  font-size: 10px; cursor: help; margin-left: 4px; vertical-align: middle;
  transition: color .14s, border-color .14s; }
.cfg-q:hover { color: var(--fg); border-color: var(--border-hover); }
```
- **inline**：控件后追加 `<span class="cfg-hint">说明</span>`（`.cfg-field` 是 flex column，自然落在控件下方一行）。
- **tooltip**：字段名后追加 `<span class="cfg-q" title="说明" aria-label="说明">?</span>`（原生 `title` 即 hover 提示）。

### 3.4 a11y
`?` 用 `title` 提供 hover 提示并加 `aria-label`（屏幕阅读器可读）。inline `.cfg-hint` 可选用 `aria-describedby` 关联控件（增强；实施时若不增成本则一并做，否则单 `.cfg-hint` 文本本身已在 DOM 顺序紧邻控件）。

## 4. 逐字段归类 + 文案（i=inline，t=tooltip）

### `[retrieval]`
| 字段 | 形式 | 文案 |
| --- | --- | --- |
| `strategy` | i | 检索策略：bm25=纯词法召回（无需 key）、vector=向量语义、hybrid=词法+向量混合、subagent=智能体规划 |
| `top_k` | i | 每次检索返回给客户端的工具条数上限 |
| `vector.model` | i | 向量化（embedding）模型名，如 text-embedding-3-small |
| `vector.api_key_env` | i | 存放 API key 的环境变量名（只填变量名，不填密钥本身） |
| `vector.base_url` | t | 向量化服务的 API 基地址；留空用内置默认 |
| `vector.dim` | t | 向量维度，需与所选模型匹配（可选） |
| `vector.timeout_ms` | t | 单次向量化请求的超时（毫秒） |
| `vector.batch_size` | t | 批量向量化时每批的条数 |
| `subagent.model` | i | 规划用 LLM 模型名 |
| `subagent.api_key_env` | i | 存放 API key 的环境变量名（只填变量名） |
| `subagent.base_url` | t | LLM 服务 API 基地址；留空用默认 |
| `subagent.timeout_ms` | t | 单次规划请求超时（毫秒） |
| `subagent.candidates` | t | 每轮候选工具数（可选） |

### `[server]`
| 字段 | 形式 | 文案 |
| --- | --- | --- |
| `stdio` | i | 是否开启 stdio 传输（供本地 MCP 客户端经标准输入输出连接） |
| `http.enabled` | i | 是否开启 HTTP（Streamable HTTP）传输 |
| `http.bind` | i | HTTP 监听地址，host:port，如 127.0.0.1:8970 |
| `http.path` | t | MCP 端点路径，默认 /mcp |
| `http.api_key[].name` | i | key 的标签（仅用于日志/观测，非密钥本身） |
| `http.api_key[].env` | i | 存放该 key 的环境变量名 |

### `[audit]`
| 字段 | 形式 | 文案 |
| --- | --- | --- |
| `enabled` | i | 是否开启调用审计（落 JSONL） |
| `path` | i | 审计 JSONL 文件路径 |

### `[dashboard]`
| 字段 | 形式 | 文案 |
| --- | --- | --- |
| `enabled` | i | 是否开启可视化面板 |
| `bind` | i | 面板监听地址，host:port，如 127.0.0.1:8971 |
| `trace_queries` | i | 是否捕获 query→tools 的检索追踪（供面板回放） |
| `trace_path` | t | 检索追踪 JSONL 路径（配了才有「历史」回放，可选） |
| `trace_buffer` | t | 内存中保留的检索追踪条数 |
| `call_buffer` | t | 内存中保留的调用记录条数 |
| `payload_max_bytes` | t | 单条调用 args/result 入环的字节上限 |
| `admin_token_env` | t | admin 写操作 Bearer token 的环境变量名（不配则写 API 全 404，可选） |
| `disabled_state_path` | t | 运行时禁用集的持久化文件路径（可选） |

### `[[upstream]]`
| 字段 | 形式 | 文案 |
| --- | --- | --- |
| `name` | i | 该上游工具的命名空间前缀；非空、唯一、不含 `__` |
| `transport` | i | 连接方式：stdio=本地子进程、http=远程 Streamable HTTP |
| `command`（stdio） | i | 子进程可执行文件路径 |
| `url`（http） | i | 远程 MCP 端点 URL，如 https://…/mcp |
| `call_timeout_ms` | t | 单次工具调用超时（毫秒，默认 30000） |
| `args`（stdio） | t | 子进程启动参数（空格分隔；含空格的参数请用 raw 模式） |
| `env_passthrough`（stdio） | t | 透传给子进程的环境变量名（其余环境被清空） |
| `bearer_env`（http） | t | 存放 Bearer token 的环境变量名（→ Authorization: Bearer，可选） |
| `headers`（http） | t | 自定义请求头：header 名 → 存放其值的环境变量名 |

## 5. 范围（实施边界）

- **`crates/dashboard/ui/src/lib/Section{Retrieval,Server,Audit,Dashboard,Upstreams}.svelte`**：每个字段 `<label class="cfg-field">` 内追加 inline `.cfg-hint` 或 `?` `.cfg-q`（按上表），只加元素、不改 `bind:`/`onchange`/`oninput`/逻辑。
- **`crates/dashboard/ui/src/app.css`**：加 `.cfg-hint` + `.cfg-q` 规则（mock 已验证）。
- **重建并提交 `dist/`**（字节级可复现）。
- 后端 / 校验 / 同步 / `configSchema` 逻辑：**不动**。

## 6. 验收 / Gates

- `npm run test` → **28 passed**（逻辑零改动）。
- `npm run build` exit 0；`npm ci && npm run build` 后 `git status dist` 空（可复现）。
- 后端 `cargo build --locked` + `cargo test`（328）不受影响（前端-only）。
- **视觉手测**：每段每字段都有说明；inline/tooltip 划分符合原则 A；`?` hover 出提示；深色下与表单整体一致、不喧宾夺主。

## 7. 风险与缓解

- **说明文案不准**：文案基于 `crates/config/src/lib.rs` 各字段语义撰写；实施时对照结构体注释核对。
- **inline 让表单变长**：只对必填/关键字段用 inline（数量受控），其余 tooltip 收起，整体高度可控。
- **dist 漂移**：组件/CSS 改动后重建并提交 dist，最终 `npm ci && build` 验证可复现。
