# M2.T5 设计：SubagentStrategy（BM25 预筛 → 小模型重排）

> 状态：已通过 brainstorm 评审，待 writing-plans 细化为实现计划。
> 前置：M0 / M1（A/B.1/B.2/C）/ M2-A / M2-B 已合并到 master（HEAD `9b0edb0`）。
> 关联里程碑：roadmap `M2.T5 — SubagentStrategy（可选）`。

## 1. 目标与范围

新增可选的 **`"subagent"`** 检索策略：先用 **BM25 快速预筛**出候选 shortlist，再交给一个**便宜的小模型**
（Haiku/Flash 等）在候选里**重排/挑选** top-k（retrieve-then-rerank）。opt-in，**默认仍是 `bm25`**。

**范围内：**
- `retrieval`（无 HTTP）新增通用 **`ChatModel`** 抽象 + `ChatError` + `MockChatModel`（testkit）。
- 新增 **`SubagentStrategy`**：内置 `Bm25Strategy` 预筛 + `ChatModel` 重排，**失败透明降级到 BM25**。
- 新增 **`chat` crate**（工作区第二个、也是仅有的另一个带 HTTP 依赖的 crate）：`OpenAiChat` 调
  OpenAI 兼容 `/chat/completions`，实现 `retrieval::ChatModel`。
- **后端注入重构**：`build_strategy(name, &Backends)`，`Backends { embedder, chat }`；连带改 `GatewayState`、
  `mcpgw` 装配、所有调用点与现有测试。
- config 新增 `[retrieval.subagent]`；`strategy = "subagent"` 落地即可用。

**明确不含（YAGNI / 留后续）：**
- **不切换默认策略**：默认仍 `bm25`（subagent 依赖云端 LLM + key，不能零配置默认）。
- 预筛可配置（vector/hybrid 预筛）——v1 **固定 BM25 预筛**，只依赖一个 LLM、自洽。
- 多轮 / 工具调用 / 流式 / 函数调用；候选去重以外的 prompt 调优；本地 LLM。
- 把 prompt/解析逻辑下沉进 `chat` crate——**prompt 构造与响应解析留在 `retrieval`**（纯逻辑、可 mock 测）。

## 2. 决策记录（brainstorm 已敲定）

| 决策点 | 结论 | 理由 |
|--------|------|------|
| 核心机制 | **召回后重排（B）**：BM25 预筛 → 小模型重排 | 提示词有界、成本/延迟可控；复用现成 BM25 |
| 前置召回 | **固定 BM25**（v1） | 只依赖一个 LLM、自洽；语义预筛留后续 |
| 后端注入 | **`Backends` 结构体** | 面向未来：再加后端不改 `build_strategy` 签名 |
| LLM 客户端落点 | `retrieval` 定 `ChatModel` trait；HTTP 实现在新 `chat` crate | 镜像 `embedder` 模式，retrieval 保持无 HTTP |
| prompt/解析位置 | 在 `retrieval`（`SubagentStrategy` 内） | 纯逻辑、用 `MockChatModel` 确定性测试 |
| 默认策略 | **仍 `bm25`** | subagent 需云端 LLM + key，不能零配置默认 |
| LLM 失败 | **透明降级到 BM25 shortlist** | 与 `VectorStrategy → BM25` 一致，永不硬失败 |

## 3. crate 划分

| crate | 新增/改动 |
|------|-----------|
| `retrieval` | 新增 `ChatModel` trait + `ChatError` + `MockChatModel`(testkit)；新增 `SubagentStrategy`；`StrategyError` 加 `ChatModelRequired`；`build_strategy` 改签名为 `(name, &Backends)`；新增 `Backends` 结构体；re-export |
| `chat`（**新 crate**）| `OpenAiChat`：reqwest 调 `POST {base_url}/chat/completions`，实现 `retrieval::ChatModel`。HTTP/序列化只在此 crate |
| `config` | `[retrieval.subagent]` 结构（base_url/model/api_key_env/timeout_ms?/candidates?）+ 校验；`KNOWN` 加 `"subagent"` |
| `gateway` | `GatewayState` 持有 `Backends`（替 `Option<Arc<dyn Embedder>>`）；`with_embedder` → `with_backends`；rebuild 传 `&Backends` |
| `mcpgw` | `build_embedder` → `build_backends`（按 strategy 建 embedder 和/或 chat）→ 注入；CLI `search` 离线无后端，subagent 报 `ChatModelRequired` |
| `Cargo.toml`(workspace) | members 加 `crates/chat` |

> `chat` 引入 reqwest+serde（第二个 HTTP 用户，与 `embedder` 对称）。`retrieval` **仍不** 依赖 reqwest。

## 4. 抽象与数据流

### 4.1 `ChatModel`（retrieval，async）
```rust
#[async_trait]
pub trait ChatModel: Send + Sync {
    /// 一次聊天补全：system + user 提示 → assistant 文本。
    async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("chat provider error: {0}")]
    Provider(String),   // 网络/HTTP/非 2xx
    #[error("chat model returned no usable content")]
    Empty,              // 无 choices / content 为空
}
```
- 通用、单轮、无状态——也为后续（M5 审批推理、M6 code-mode）留口。
- `MockChatModel`（**仅 testkit**）：可脚本化——成功返回给定字符串（如一段 JSON），或 `failing()` 永远 `Err`。
  记录 `calls` 与最近一次 `system`/`user`，供 prompt 断言。

### 4.2 `SubagentStrategy`
```rust
pub struct SubagentStrategy {
    bm25: Bm25Strategy,
    chat: Arc<dyn ChatModel>,
    candidates: usize,   // 预筛 shortlist 大小（默认 20）
}
impl SubagentStrategy {
    pub fn new(chat: Arc<dyn ChatModel>, candidates: usize) -> Self;
}
```
- `index(catalog)`：建内部 `bm25`（其 `ScoredTool` 已含 `qualified_name` + `description`，足够构造 prompt，无需另存目录）。
- `search(query, top_k)`：
  1. `shortlist = bm25.search(query, candidates)`。**`shortlist` 为空 → 返回空**（无字面命中，无可重排）。
  2. 构造 prompt（见 4.3），`chat.complete(system, user).await`。
  3. 解析出**有序工具名列表**；**只保留出现在 shortlist 里的名字**（丢幻觉）、去重、保序。
  4. 映射成 `ScoredTool`（按名次给**递减合成分**，例如 `score = (n - i) as f32`，仅用于本策略内排序），截 `top_k`。
  5. **降级**：`complete` 出错、解析失败、或**零有效名** → 回落到 `shortlist` 截 `top_k`（BM25 名序），记 `tracing::warn!`。
- `ScoredTool.score` 为合成名次分，**不可跨策略比较**（与 hybrid 的 RRF 分同理）。

### 4.3 Prompt 与解析（在 retrieval，可 mock 测）
- **system**：固定一句——「你是工具选择器。给定用户查询与编号候选工具清单，返回一个 **JSON 字符串数组**，
  列出最相关的工具**限定名**，最相关在前，最多 N 个，**只能从候选里选**，不要解释。」
- **user**：查询 + 编号候选清单，每行 `i. {qualified_name}: {description}`。
- **解析**（鲁棒、保守）：
  - 截取响应中第一个 `[` 到与之匹配的 `]`，按 JSON 字符串数组解析；
  - 过滤：只保留**精确等于** shortlist 中某 `qualified_name` 的项；去重、保序；
  - 解析失败 / 数组空 / 过滤后为空 → 触发**降级**（回 BM25 shortlist）。
- 解析逻辑放 retrieval，`MockChatModel` 喂入各种响应即可单测（合法 JSON、含幻觉名、乱码、空）。

### 4.4 `chat` crate（HTTP 实现，镜像 `embedder`）
```rust
pub struct OpenAiChat { /* client, base_url, model, api_key, ... */ }
impl OpenAiChat { pub fn new(base_url, model, api_key, timeout: Option<Duration>) -> Self; }
#[async_trait] impl retrieval::ChatModel for OpenAiChat { ... }
```
- `complete`：`POST {base_url}/chat/completions`，body `{ model, messages: [{role:"system",..},{role:"user",..}], temperature: 0 }`，
  `Authorization: Bearer {api_key}`。取 `choices[0].message.content`；无 choices/空内容 → `ChatError::Empty`；
  非 2xx → `ChatError::Provider`（附**截断**的 body 片段，与 `OpenAiEmbedder` 一致）。`temperature=0` 求稳定。

### 4.5 后端注入：`Backends` 与 `build_strategy`
```rust
#[derive(Default, Clone)]
pub struct Backends {
    pub embedder: Option<Arc<dyn Embedder>>,
    pub chat: Option<Arc<dyn ChatModel>>,
}
pub fn build_strategy(name: &str, backends: &Backends)
    -> Result<Box<dyn RetrievalStrategy>, StrategyError>;
```
- 臂：`"bm25"`（无需后端）｜`"vector"`/`"hybrid"`（需 `embedder`，否则 `EmbedderRequired`）｜
  `"subagent"`（需 `chat`，否则 **`ChatModelRequired`**）｜其它 `NotImplemented`。
- `StrategyError` 新增 `ChatModelRequired(String)`。

## 5. 行为与边界情形

- **空 shortlist（无字面命中）→ 空结果**：这是固定 BM25 预筛的已知局限（纯语义查询召回为空），L3 文档注明。
- **幻觉过滤**：LLM 可能返回不在候选里的名字（甚至编造）——一律丢弃；若过滤后为空则降级。
- **降级自愈**：任何 LLM/解析失败都回落到 BM25 shortlist 名序，**永不硬失败**（与 VectorStrategy 一致）。
- **确定性**：给定 `MockChatModel` 的响应，结果完全确定，可 golden。真实 `OpenAiChat` 用 `temperature=0` 求稳。
- **离线 CLI**：`mcpgw search` 不注入后端，`strategy="subagent"` → `ChatModelRequired`（与 vector/hybrid 的 `EmbedderRequired` 对称）。

## 6. 配置接线

### 6.1 `[retrieval.subagent]`
```toml
[retrieval]
strategy = "subagent"
[retrieval.subagent]
base_url    = "https://api.openai.com/v1"  # 默认
model       = "gpt-4o-mini"                # 小/便宜模型
api_key_env = "OPENAI_API_KEY"             # 仅引用 env 变量名
# timeout_ms = 8000                        # 可选
# candidates = 20                          # 可选，预筛 shortlist 大小（默认 20）
```
- `validate`：`strategy=="subagent"` 时必须有 `[retrieval.subagent]`，且 `base_url`/`model`/`api_key_env` 非空；
  `candidates`（若给）须 `> 0`。`KNOWN` 白名单加 `"subagent"`。

### 6.2 `mcpgw::build_backends`
- 替换现 `build_embedder`：按 `cfg.retrieval.strategy` 决定建哪些后端，返回 `retrieval::Backends`：
  - `"vector"`/`"hybrid"` → 建 `embedder`（OpenAiEmbedder + CachingEmbedder），`chat = None`。
  - `"subagent"` → 建 `chat`（OpenAiChat），`embedder = None`；启动期 fail-fast 读 `api_key_env`。
  - 其它（`bm25`）→ `Backends::default()`（都为 None）。
- `prepare_state`：`GatewayState::with_backends(&cfg.retrieval.strategy, backends)`。

### 6.3 `gateway::GatewayState`
- 字段 `embedder: Option<Arc<dyn Embedder>>` → `backends: Backends`。
- `new(strategy)` = `with_backends(strategy, Backends::default())`；`with_embedder` 删除或保留为薄封装（实现计划定，倾向**改为 `with_backends`** 并更新现有 vector/hybrid 测试）。
- `rebuild_snapshot`：`build_strategy(&self.strategy_name, &self.backends)`。

## 7. 测试策略（含 corner + 核心；专门测试可由子代理补）

- **`subagent` 单元/集成（`MockChatModel`，testkit）**：
  - 重排生效：mock 返回 shortlist 的某个重排 JSON → 策略按该序返回。
  - 幻觉过滤：mock 返回含非候选名 → 被丢弃；只剩候选名。
  - 降级：`MockChatModel::failing` → 回落到 BM25 shortlist 名序（与独立 BM25 对比）。
  - 空 shortlist：无字面命中的 query（如 `"zzzznonexistent"`）→ 空（chat 不被调用，可由 `calls==0` 佐证）。
  - 解析失败：mock 返回乱码/空数组 → 回落 BM25。
  - `top_k` 截断；`candidates` 预筛深度（shortlist 上限）。
- **`build_strategy`**：`subagent` + 无 chat → `ChatModelRequired`；+ chat → ok 且可 index/search。
- **`config`**：`strategy="subagent"` 缺 `[retrieval.subagent]` → `Invalid`；解析 + `candidates` 校验。
- **`gateway`**：`with_backends("subagent", Backends{chat})` rebuild 建出 subagent 快照（镜像 vector/hybrid 用例）。
- **`mcpgw`**：`build_backends` 对 `"subagent"` 建出含 `chat` 的 `Backends`（env key）。
- **`chat` crate**：`OpenAiChat` 打到 mock axum server——正确取 `choices[0].message.content`；非 2xx → `Provider`；空 choices → `Empty`（镜像 `embedder` 的 openai 测试）。
- **门控真实冒烟**（`#[ignore]` / 需 key）：真实小模型对一个语义查询能挑出预期工具。
- **回归**：`Backends` 重构后，既有 bm25/vector/hybrid 路径全绿（build_strategy 签名变更的全调用点）。

## 8. 分层文档（DoD，随代码同提交）

- **L4**：新 `chat-openai.md`（`OpenAiChat`）；新 `retrieval-subagent.md`（`SubagentStrategy`）；
  更新 `retrieval-lib.md`（`ChatModel`/`ChatError`/`Backends`/`build_strategy` 新签名/`ChatModelRequired`）、
  `config-lib.md`（`[retrieval.subagent]` + validate）、`mcpgw-main.md`（`build_backends`）。
- **L3**：`retrieval.md`——subagent 数据流（预筛→prompt→解析→降级）、空 shortlist 局限。
- **L2**：`retrieval.md`（subagent + ChatModel + Backends）；新 `chat.md`（组件）；`config.md`/`mcpgw-cli.md` 按需。
- **L1**：`L1-overview.md`——新增 `chat` crate（第二个 HTTP 依赖）、subagent 策略；**默认仍 bm25**。
- **索引/路线图**：`docs/README.md` L4/L2 清单加 chat/subagent；roadmap 标 **M2.T5 ✅ 已完成**。

## 9. 实现期需现场确认/可能回退的点

- `build_strategy` 签名从 `(name, Option<&Arc<dyn Embedder>>)` 改为 `(name, &Backends)` 是**破坏性签名变更**：
  需同步 `gateway`、`mcpgw`、`retrieval` 既有 vector/hybrid 测试与 `build_strategy_*` 测试（全调用点编译实证）。
- `GatewayState` 从持有 embedder 改为持有 `Backends`：`with_embedder` 调用点（gateway/mcpgw 测试）需改为 `with_backends`。
- LLM 响应解析的鲁棒性：真实模型可能不严格输出 JSON（加解释/代码围栏）——v1 采「截取首个 `[...]` + 候选名白名单过滤」，
  失败即降级；若真实冒烟显示需要更宽松解析，再迭代（保持「失败必降级」不变）。
- `candidates` 默认 20 是否合适：太小漏召回、太大提示词膨胀；实现期可按真实冒烟微调默认值。
- 合成名次分仅用于本策略内排序与 `search_tools` 的 `top_k` 截断；downstream 只用顺序、丢弃分值（与 hybrid 一致）。
