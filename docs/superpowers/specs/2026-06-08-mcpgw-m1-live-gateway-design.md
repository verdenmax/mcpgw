# mcpgw M1 —— 活 MCP 网关（I/O 层）设计文档

- **状态**: 已批准设计，待编写实现计划
- **日期**: 2026-06-08
- **里程碑**: M1（见 `docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md`）
- **承接**: M0 检索核心（`2026-06-08-mcpgw-retrieval-core.md`）；总设计 `2026-06-08-mcpgw-progressive-discovery-design.md`

---

## 1. 目标与背景

M0 交付了依赖最少的检索内核（catalog / retrieval / config / mcpgw-cli），但它还不是一个能用的网关。
**M1 把它变成真正的活 MCP 网关**：任意 MCP 客户端连上 mcpgw → 只看到 `search_tools` /
`get_tool_details` / `call_tool` 三个固定元工具 → 背后聚合 N 个真实上游 MCP 服务器。

这把"渐进式工具发现（progressive tool discovery）"做在**网关/代理层（server 侧）**，于是任何客户端
零改造即可享受按需加载，从根本上避免"把上百个工具一次性塞给 LLM"导致的上下文爆炸。

## 2. 范围

### 2.1 M1 范围（已敲定的决策）

| 决策点 | 决策 |
|--------|------|
| 协议 SDK | **rmcp**（官方 Rust MCP SDK）做 client + server，不自撸协议 |
| 传输 | **stdio + Streamable HTTP**，上游与下游全覆盖 |
| 聚合能力 | **只 tools**（三个元工具）；resources / prompts 透传推迟 |
| 上游连接 | **eager**：启动异步预连所有上游，就绪即入目录，单挂不影响其它 |
| 下游鉴权 | 默认绑 **localhost** + 可选 **静态 API-Key（Bearer，env 读）**；完整 OAuth 留 M3 |
| 并发模型 | **ArcSwap 快照（build-then-swap）** |

### 2.2 非目标（推迟）

resources/prompts 聚合、lazy 上游连接 + dynamic server management、完整 OAuth/DCR/反代正确性（M3）、
向量/混合/subagent 检索（M2）、Web 控制面板（M4）、RBAC（M5）、可观测/审计/code-mode（M6）。

## 3. rmcp 能力核实（M1.T1 选型结论）

rmcp 已覆盖 M1 所需的一切，故**直接采用 rmcp**，不再自撸协议：

- **Client**：`().serve(TokioChildProcess::new(Command::new("npx")…))` 拉起 stdio 子进程上游；
  `transport-streamable-http-client` 连 HTTP 上游；`client.list_all_tools()` / `call_tool`。
- **Server**：实现 `ServerHandler` trait，`.serve(stdio())` 或 `transport-streamable-http-server`（axum）。
- **通知**（`notifications/tools/list_changed`）、**OAuth**（`auth` feature，留 M3）、`which-command`
  （解析 npx/uvx 可执行路径）均内置；测试充分（含 spawn JS/Python 上游 + streamable http）。

相关 feature（按需启用）：`client`, `server`, `macros`, `transport-child-process`, `transport-io`,
`transport-streamable-http-client(-reqwest)`, `transport-streamable-http-server`, `server-side-http`,
`which-command`。

## 4. 架构与 crate 布局

新增 4 个 crate，扩展 `config`/`mcpgw`，依赖方向无环：

```
crates/
├─ upstream/    # rmcp client：连上游(stdio/HTTP)、生命周期、工具摄取、list_changed、call 转发
├─ metatools/   # 三个元工具实现 + 调用路由；依赖 catalog/retrieval/upstream
├─ downstream/  # rmcp ServerHandler：暴露三元工具(stdio + axum HTTP)、API-Key；依赖 metatools
├─ gateway/     # 编排器：持有 ArcSwap 快照 + CatalogManager 循环 + 装配 registry/server；依赖以上全部
├─ config/      # + [server]、[[upstream]]（M0 已有 [retrieval]）
└─ mcpgw/       # + serve 子命令；薄 CLI 调 gateway::run(config)
```

依赖：`downstream → metatools → {catalog, retrieval, upstream}`；`gateway → 全部`；`mcpgw → gateway`。

## 5. 组件职责

### 5.1 `upstream`
- `UpstreamManager`：对每个 `[[upstream]]` **eager 异步**建立 rmcp client 连接；指数退避重连；健康状态
  （Connecting / Ready / Failed）；订阅 `notifications/tools/list_changed`。
- 把上游 `Tool` 映射为 `catalog::ToolDef`：`server = upstream.name`、`name = tool.name`、
  `description`、`input_schema`（命名空间 = `{server}__{name}`）。
- `UpstreamRegistry`：按 server 名索引活动连接句柄，供 `call_tool` 转发；连接状态变化时通知
  `CatalogManager`（channel）。

### 5.2 `metatools`
基于共享句柄（`Arc<ArcSwap<GatewaySnapshot>>` + `Arc<UpstreamRegistry>`）实现三个纯异步函数：
- `search_tools(query, top_k?)` → 读当前快照 → `strategy.search` → 返回 `[{ name, description }]`。
- `get_tool_details(name)` → `snapshot.catalog.get(name)` → 完整 `input_schema` 等。
- `call_tool(name, arguments)` → 解析命名空间名 `{server}__{tool}` → `registry.get(server)` →
  上游 `call_tool(tool, arguments)`（带可配置超时）→ 透传结果（含 `isError`）。

### 5.3 `downstream`
- `GatewayServer` 实现 rmcp `ServerHandler`：`list_tools` 恒返回 3 个元工具及其固定 JSON schema；
  `call_tool` 按名派发到 `metatools`。
- 传输：`serve(stdio())`；以及 Streamable HTTP（rmcp `server-side-http` + axum），可选 API-Key
  Bearer 中间件；绑定地址来自 `[server.http]`。

### 5.4 `gateway`
- 持有 `Arc<ArcSwap<GatewaySnapshot>>`，`GatewaySnapshot = { catalog: Catalog, strategy: Box<dyn RetrievalStrategy> }`。
- `CatalogManager` 后台任务：接收来自 `UpstreamManager` 的"工具变化"事件（初次就绪、`list_changed`、
  重连、断开），**重建**合并后的 `Catalog`、新建并 `index` 策略、`ArcSwap::store` 新快照（原子替换）。
- `run(config)`：装配 UpstreamManager（eager 启动）、构造初始快照、按 `[server]` 启动下游 stdio/HTTP
  服务，运行至关闭。

## 6. 并发 / 状态模型（ArcSwap 快照）

- 检索读端（`search_tools` / `get_tool_details`）只 `ArcSwap::load()` 当前快照 → **无锁读，刷新永不
  阻塞搜索**。
- 写端（`CatalogManager`）在上游工具变化时构建**全新快照**（克隆/重建 catalog + 新建并 index 策略），
  原子 `store`。
- 上游连接句柄放 `UpstreamRegistry`（成员仅在连接/断开时变化）；`call_tool` 按 server 名查句柄转发。
- **不改 `RetrievalStrategy` trait**：快照里"新建策略实例 → `index(&catalog)` → 装进 Arc → swap"即可，
  Arc 后的策略只读 `search(&self)`。

## 7. 数据流（`mcpgw serve`）

```
启动: 读 config → UpstreamManager 异步连所有上游 → 每个就绪后 tools/list
      → CatalogManager 合并 → 新建+index 策略 → ArcSwap.store(快照)
      并行: downstream server(stdio/HTTP) 启动，开始接客户端
search_tools(query, top_k?)  → ArcSwap.load() → strategy.search → [{name, description}]
get_tool_details(name)       → 快照.catalog.get → 完整 schema
call_tool(name, args)        → UpstreamRegistry.get(server) → 上游 tools/call → 回传(透传 isError)
上游 list_changed            → 重取该上游工具 → CatalogManager 重建快照 → swap
```

**关键不变量**：下游 `tools/list` 永远只有 3 个元工具、稳定不变（与上游状态无关）→ prompt 缓存友好、
全客户端兼容；"当下相关工具"仅通过 `search_tools` 结果体现。

## 8. 配置 Schema（扩展）

```toml
[server]
stdio = true                          # 对下游暴露 stdio
[server.http]
enabled = true
bind = "127.0.0.1:8970"               # 默认 localhost
api_key_env = "MCPGW_API_KEY"         # 可选；设了则要求 Bearer

[retrieval]                           # M0 已有
strategy = "bm25"
top_k = 8

[[upstream]]
name = "github"                       # 命名空间前缀（禁止含 "__"）
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_passthrough = ["GITHUB_TOKEN"]    # 仅透传白名单环境变量
call_timeout_ms = 30000               # 可选，默认 30s

[[upstream]]
name = "search"
transport = "http"
url = "https://example.com/mcp"
# headers_env = "SEARCH_MCP_HEADERS"  # 可选鉴权头，env 读
```

- **策略白名单单一来源（收编 M0 遗留项①）**：`config` 不再硬编码策略名校验；只做类型/非空检查，
  `serve` 启动时由 `retrieval::build_strategy` 作为唯一权威给出"未实现"错误。
- **密钥原则**：所有 token / api-key / headers 仅经 env 引用，绝不写入配置或日志。
- **命名空间校验**：`upstream.name` 禁止含 `__`。

## 9. 错误处理（分层、隔离、自愈）

| 场景 | 处理 |
|------|------|
| 上游连接/启动失败 | 标记 Failed、其工具排除出快照、指数退避重连；**不持久化硬失败**（避开 MetaMCP #264 卡死） |
| `call_tool` 打到缺失/失败上游 | 返回 MCP 结构化错误 `isError:true`，让模型自我纠正 |
| `call_tool` 超时（可配置） | `isError`，带超时信息 |
| 上游工具重名 | 命名空间前缀强制隔离；摄取时**显式冲突检测**（收编遗留项④：warn） |
| 下游 HTTP 鉴权失败 | 401 |
| 配置非法 | `serve` 启动即 fail-fast，明确字段错误 |
| 部分可用 | 网关只服务已就绪上游；下游 `tools/list` 恒为 3 元工具，与上游状态解耦 |

## 10. 测试策略

- **Mock 上游 MCP**：用 rmcp 起一个暴露若干静态工具的小型上游（in-memory duplex 或 stdio child），
  驱动集成测试。
- **端到端（stdio + HTTP 各一遍）**：mock 上游 → 启动 gateway → rmcp 测试客户端连上 → 断言
  `search_tools → get_tool_details → call_tool` 全链路。
- **`list_changed` 重建**：mock 上游变更工具列表并发通知 → 断言快照更新（`search_tools` 搜到新工具）。
- **隔离**：一个上游崩溃 → 其它仍可搜/可调；下游 `tools/list` 稳定。
- **HTTP 鉴权**：设了 `api_key_env` → 无 Bearer 401、有 Bearer 200。
- **单元**：`Tool`→`ToolDef` 映射与命名空间；`call_tool` 名解析（`server__name`→server+原名）；快照
  重建；命名冲突检测。
- **文档（L1–L4）**：每个 task 同步对应层级文档（新 crate → L1/L2/L3/L4），随代码提交，纳入双重审查验收。

## 11. 收编的 M0 遗留项

- ①策略白名单单一来源（config ↔ build_strategy）→ 第 8 节处理。
- ③ build-then-swap 并发 → 第 6 节状态模型即是。
- ④上游去重/命名冲突检测 → 摄取时 warn（第 9 节）。
- （②CLI 默认 catalog 路径属离线调试命令，M1 不涉及。）

## 12. 成功标准

- 聚合 ≥2 个上游（stdio + HTTP 各一），客户端仅见 3 个元工具，`tools/list` 稳定不变。
- 在 stdio 与 HTTP 两种下游传输上各跑通一次 `search → inspect → execute` 全链路。
- 上游 `list_changed` 后，`search_tools` 能搜到新增/变更工具（快照已重建）。
- 单个上游崩溃不影响其它上游的检索与调用；下游始终在线。
- HTTP 端在设置 API-Key 后正确拒绝无凭据请求。
- 相比"全量塞工具"，客户端侧工具定义 token 占用显著下降（仅 3 个元工具）。

## 13. 开工前仍需在实现计划里细化的点

- rmcp 具体版本与 `ServerHandler` / client 的精确 API 形状（计划首个 task 做最小 spike 固化）。
- `UpstreamRegistry` 的具体并发原语（RwLock<HashMap> vs ArcSwap）与句柄可克隆性。
- 下游同时跑 stdio + HTTP 时的进程模型（两个 server 任务并行）。
- Mock 上游用 in-memory duplex 还是真实 stdio child（影响测试速度与真实度）。
