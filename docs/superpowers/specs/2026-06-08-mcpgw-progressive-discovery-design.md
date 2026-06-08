# mcpgw —— 智能 MCP 网关设计文档

- **状态**: 已批准设计，待编写实现计划
- **日期**: 2026-06-08
- **工作代号**: `mcpgw`（名称待定）
- **作者**: @verdenmax（与 Copilot 协作 brainstorming）

---

## 1. 背景与动机

随着 MCP（Model Context Protocol）生态扩张，用户需要把多个本地/远程 MCP server
聚合到一起使用。现有工具（MetaMCP、tbxark/mcp-proxy、moxy、mcp-meta-hub 等）已能做
"聚合成单端点"，但存在共性短板：

- **工具爆炸 / 上下文膨胀（核心痛点，无人真正解决）**：聚合后所有 tool 的 schema
  一次性塞给 LLM，几十上百个工具 → 上下文爆炸 + 选错工具。各项目宣传的"智能工具检索"
  普遍标注 *coming soon* 或半成品。
- **公网暴露 + 鉴权 bug 密集**：反代后 OAuth DCR 返回 `localhost`、CSRF state 不校验、
  OAuth 竞态等（参考 MetaMCP issues #263/#265/#277/#296/#298/#299）。
- **资源/并发问题**：STDIO server 每 namespace 重复 spawn、并行调用被串行化
  （MetaMCP #272/#304）。
- **可用性**：纯配置文件派无 Web UI；有 UI 的桌面端凑合、无移动端。
- **安全/治理**：密钥裸奔、缺细粒度 RBAC、缺危险操作审批、缺审计。
- **维护可持续性**：头部项目 bus factor ≈ 1，社区治理薄弱。

本项目聚焦**最痛、最有差异化空间**的一项作为核心切入点：**智能工具检索 / 按需加载**。
其余作为后续迭代（见第 9 节路线图）。

---

## 2. 目标与非目标

### 2.1 核心目标（MVP / Phase 1）

把"渐进式工具发现"做在**代理/网关层（server 侧）**，使**任何 MCP 客户端零改造**即可
享受按需加载，从根本上解决工具爆炸与上下文膨胀。

### 2.2 非目标（推迟到后续 Phase，已纳入路线图）

公网暴露 UX、Web/移动控制面板、细粒度 RBAC + 审批、全链路可观测/审计、
programmatic/code-mode 沙箱执行、本地 embedding、subagent 检索策略。

---

## 3. 关键设计决策与依据

| 决策点 | 决策 | 依据 |
|--------|------|------|
| **核心交互模型** | **模型 A：元工具 / 渐进式披露**——只暴露 `search_tools` / `get_tool_details` / `call_tool` 三个固定元工具 | MCP 官方《Client Best Practices》明确推荐 Progressive Tool Discovery；prompt 缓存友好（工具数组不变）；兼容所有客户端 |
| **`list_changed` 定位** | 仅用于网关**内部重建检索索引**，**不**用于改变模型可见工具列表 | 协议中 `list_changed` 为可选能力；Claude Desktop / Cursor 等主流客户端运行中刷新不可靠；官方仍以 SEP-2549(TTL)/SEP-2567 解决缓存陈旧 |
| **检索策略** | **可插拔多策略**（BM25 / 向量 / Hybrid / 后续 subagent），后端配置选择，**默认 BM25+向量 Hybrid**；v1 先落 BM25，向量紧随其后 | 用户要求"几个都做且可配置"；官方列出 keyword/embedding/subagent/hybrid 四种 |
| **Embedding 来源** | 云 API（如 OpenAI `text-embedding-3-small`）；本地 embedding 作后续 | 用户偏好；本项目不做模型训练，向量化即 HTTP 调用 |
| **技术栈** | **Rust** 做核心网关（tokio / axum / tantivy / rmcp）；检索策略**先用 Python 脚本快速验证调参，再下沉到 Rust** | 代理本质是异步 I/O 多路复用，Rust 单二进制 / 低内存 / 高并发直击现有工具资源痛点；脚本先行降低检索迭代成本 |

### 为什么是模型 A（而非动态工具列表 + list_changed）

`tools/list_changed` 在 MCP 规范中对 server 和 client 都是 **MAY**（可选）。实测主流客户端：

| 客户端 | 启动枚举工具 | 运行中响应 list_changed |
|--------|:---:|:---:|
| Claude Desktop | ✅ | ⚠️ 弱 / 常需重启 |
| Cursor | ✅ | ⚠️ 弱 / 需手动 toggle |
| Claude Code (CLI) | ✅ | 🔶 部分 |
| VS Code (Copilot MCP) | ✅ | ✅ 较好 |
| Cline / Roo | ✅ | 🔶 重连时重取 |
| 自研 agent / LangChain | 看实现 | ❌ 多数不订阅 |

官方《Client Best Practices》在 Prompt Caching 一节**点名建议**："把所有调用走一个固定的
`call_tool({name,args})` 元工具，让 tools 数组永不变化"——这正是模型 A。因此模型 A 同时获得
**全客户端兼容** + **prompt 缓存保持** + **官方背书**三重收益。

---

## 4. 架构总览

```
                 ┌─────────── 下游客户端 (Claude / Cursor / Cline / 自研 agent) ───────────┐
                 │   只看到 3 个固定元工具：search_tools / get_tool_details / call_tool      │
                 └───────────────────────────┬───────────────────────────────────────────────┘
                          stdio  /  Streamable HTTP
                                              │
        ┌─────────────────────────────────── mcpgw (Rust) ───────────────────────────────────┐
        │   ① Downstream Server  ──►  ② Meta-Tool Layer  ──►  ⑤ Retrieval Engine (可插拔)      │
        │   (rmcp + axum)             (3 个元工具的实现)        ├ Bm25Strategy (tantivy)        │
        │                                   │                  ├ VectorStrategy (云 embedding)  │
        │                                   │                  └ HybridStrategy (RRF 融合)      │
        │                                   ▼                                                   │
        │   ④ Tool Catalog/Registry  ◄───────────────  ③ Upstream Manager                       │
        │   (聚合 + 命名空间 + 索引源)                   (tokio：stdio 子进程 / HTTP / SSE 客户端) │
        │                                   │                  + 生命周期/健康/重连/list_changed   │
        │                                   ▼                                                   │
        │   ⑦ Router/Dispatcher ── call_tool 路由回对应上游 ── tools/call                        │
        │   ⑥ Config (TOML)                                                                     │
        └──────────────────────────────────────────────────────────────────────────────────────┘
                                              │
                ┌──────────────┬──────────────┴───────────────┬──────────────┐
            上游 MCP A       上游 MCP B (HTTP)            上游 MCP C (stdio)   ...
```

---

## 5. 组件职责

每个组件单一职责、通过明确接口通信、可独立理解与测试。

| # | 组件 | 职责 | 依赖 |
|---|------|------|------|
| ① | **Downstream Server** | 对客户端实现 MCP server；`tools/list` 只返回 3 个元工具；接 stdio + Streamable HTTP | rmcp, axum |
| ② | **Meta-Tool Layer** | 实现 `search_tools` / `get_tool_details` / `call_tool`，编排检索与路由 | ④⑤⑦ |
| ③ | **Upstream Manager** | 作为 MCP client 连 N 个上游；管理进程/连接生命周期、健康检查、退避重连、订阅 `list_changed` | tokio |
| ④ | **Tool Catalog** | 聚合各上游的 tool/prompt/resource，加 server 前缀命名空间，作为检索索引与路由的唯一真相源 | — |
| ⑤ | **Retrieval Engine** | `RetrievalStrategy` trait + BM25/Vector/Hybrid 实现；配置选择；按 catalog 建/重建索引 | tantivy, 云 embedding |
| ⑥ | **Config** | TOML：上游列表、策略选择与参数、embedding 提供商/密钥引用、监听端口 | — |
| ⑦ | **Router** | `call_tool` 把命名空间名映射回"上游 + 原始工具名"，转发 `tools/call`，回传结果 | ③④ |

### 元工具接口契约

- `search_tools(query: string, top_k?: number, detail?: "name" | "name_desc")`
  → `[{ name, description }]`（默认仅名字 + 一行描述，省 token）
- `get_tool_details(name: string)` → 该工具完整 `inputSchema` / `outputSchema` / 文档
- `call_tool(name: string, arguments: object)` → 转发结果（含错误 `isError`）

---

## 6. 核心数据流

- **启动**：读 config → 连上游 → 各自 `tools/list` → 建命名空间 catalog → 建检索索引
- **搜索**：`search_tools(query)` → `strategy.search()` → 返回 top-K `{name, 一行描述}`
- **详情**：`get_tool_details(name)` → catalog 查 → 返回该工具完整 schema
- **执行**：`call_tool(name, args)` → Router → 上游 `tools/call` → 回传
- **刷新**：上游 `notifications/tools/list_changed` → 只重取该上游工具 → 更新 catalog → 重建索引

---

## 7. 配置 Schema（TOML）

```toml
[server]
# 下游暴露给客户端的传输
stdio = true
http = { enabled = true, bind = "127.0.0.1:8970" }

[retrieval]
strategy = "hybrid"          # bm25 | vector | hybrid | subagent(后续)
top_k = 8
detail_levels = true         # 支持 name-only / +desc / full-schema

[retrieval.bm25]
# tantivy 参数（分词、字段权重等）

[retrieval.vector]
provider = "openai"          # 云 embedding 提供商
model = "text-embedding-3-small"
api_key_env = "OPENAI_API_KEY"   # 仅引用环境变量名，绝不写明文

[retrieval.hybrid]
fusion = "rrf"               # Reciprocal Rank Fusion

[[upstream]]
name = "github"              # 命名空间前缀
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_passthrough = ["GITHUB_TOKEN"]

[[upstream]]
name = "search"
transport = "http"
url = "https://example.com/mcp"
```

**密钥原则**：所有 API key / token 一律走环境变量引用，**绝不明文进配置文件或日志**。

---

## 8. 错误处理与测试

### 8.1 错误处理（分层、隔离、可自愈）

| 场景 | 处理 |
|------|------|
| 上游启动/连接失败 | 隔离：标记不可用、catalog 排除、指数退避重连；**不持久化硬失败状态**（避开 MetaMCP #264 卡死） |
| `call_tool` 打到死上游 | 返回 MCP 结构化错误 `isError:true`，让模型自我纠正 |
| embedding API 失败 | 自动降级到 BM25，记一条 warn（多策略的天然韧性） |
| 上游工具重名 | 命名空间前缀强制隔离；冲突时日志告警 |
| 配置非法 | 启动即 fail-fast，给出明确字段/行号错误 |

### 8.2 测试策略

- **单元**：命名空间映射、RRF 分数融合、config 解析、各 strategy 在固定 catalog 上的 `search()`
- **集成**：起一个 mock 上游 MCP（stdio，暴露已知工具）→ 断言 search→inspect→execute
  全链路 → 断言 `list_changed` 触发重建索引
- **检索质量（golden tests）**：固定一组工具 + 查询 + 期望 top-K；**先用 Python 脚本验证调参，
  再把算法下沉 Rust，脚本与 Rust 共用同一份 golden 数据**
- **韧性**：注入上游崩溃/超时，断言其它上游不受影响

---

## 9. 模块结构

```
mcpgw/
├─ crates/
│  ├─ protocol/    # rmcp 类型封装、MCP 消息
│  ├─ downstream/  # server: stdio + http 传输
│  ├─ upstream/    # client: 连接/传输/生命周期
│  ├─ catalog/     # 聚合 + 命名空间 + registry
│  ├─ retrieval/   # RetrievalStrategy trait + bm25/vector/hybrid
│  ├─ metatools/   # 三个元工具
│  └─ config/      # TOML 加载与校验
├─ scripts/        # Python：检索策略快速验证 + golden 数据生成
└─ src/bin/mcpgw.rs  # 装配与启动
```

---

## 10. 路线图（核心之后的迭代）

| 阶段 | 内容 |
|------|------|
| **P1（本 spec）** | 核心网关 + 渐进式披露：stdio+HTTP 上下游、三元工具、命名空间、BM25 默认（向量紧随）、可插拔策略骨架、config、list_changed 重建索引 |
| **P2** | 向量策略下沉 Rust（fastembed-rs 本地 embedding 可选）+ subagent 策略 |
| **P3** | 稳健公网暴露（一键隧道 + 反代不踩坑 + 靠谱 OAuth） |
| **P4** | Web + 移动控制面板（分组 / 启停 / 扫码分享 / 看调用） |
| **P5** | 细粒度 RBAC + 危险操作审批拦截 |
| **P6** | 全链路可观测性 / 审计；programmatic / code-mode（沙箱执行） |

---

## 11. 成功标准

- 聚合 ≥3 个上游 MCP，客户端仅见 3 个元工具，`tools/list` 输出稳定不变。
- `search_tools` 对自然语言查询返回相关 top-K，golden tests 通过。
- search→inspect→execute 全链路在 Claude Desktop / Cursor / Claude Code 至少各验证一次。
- 单个上游崩溃不影响其它上游的工具检索与调用。
- 相比"全量塞工具"，典型会话工具定义 token 占用显著下降（目标数量级降低）。
