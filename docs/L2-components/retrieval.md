# L2 — `retrieval` 组件

## 职责

工具**排序/检索**。定义可插拔、**异步**（`#[async_trait]`）的 `RetrievalStrategy` 抽象，并提供四种实现：
M0 的默认实现自研 **BM25**（`Bm25Strategy`）、M2-A 新增的 **`VectorStrategy`**（云端嵌入余弦检索，
内置 BM25 透明降级）、M2-B 新增的 **`HybridStrategy`**（RRF 融合 BM25 + 向量），以及 M2.T5 新增的
**`SubagentStrategy`**（BM25 预筛 + 小模型重排，失败透明降级）。同时定义 `Embedder` 与 `ChatModel` 两个
provider 无关抽象（外加 `CachingEmbedder` 装饰器），其真实 HTTP 后端分别在独立的 `embedder` / `chat` crate。
只了解 `catalog` 的类型；不了解配置文件或 CLI。

> **默认策略仍是 `bm25`**：仅当配置 `strategy = "vector"` / `"hybrid"`（且提供 `[retrieval.vector]`）或
> `"subagent"`（且提供 `[retrieval.subagent]`）时才走向量/混合/subagent 路径。

## 公开接口

### 类型 `ScoredTool`
一次检索命中：`{ qualified_name: String, description: String, score: f32 }`。派生 `Debug, Clone, PartialEq`
（**不含 `Eq`/`Hash`**，因为 `score: f32` 不可全序/不可哈希）。

### trait `RetrievalStrategy: Send + Sync`（`#[async_trait]`）
可插拔策略抽象。M2-A 起两个方法均为 `async`（经 `async-trait` 装箱为 `Send` future，保持
`Box<dyn RetrievalStrategy>` 对象安全），以便实现（如向量策略）在 `.await` 上调用网络嵌入。

| 方法 | 签名 | 说明 |
|------|------|------|
| `index` | `async (&mut self, &Catalog)` | 从当前目录（重）建内部索引 |
| `search` | `async (&self, query: &str, top_k: usize) -> Vec<ScoredTool>` | 返回最多 `top_k` 条，按相关性降序 |

> `GatewayState::new` **不在构造时索引**（空目录 → 首次 `rebuild_snapshot` 前 `search` 返回空）。

### 函数 `tokenize`
`pub fn tokenize(text: &str) -> Vec<String>`：小写化、按非字母数字边界切分（`_` 也作为边界）、丢空串。
Unicode 感知（`char::is_alphanumeric` + `to_lowercase`）。

### 类型 `Bm25Strategy`
内存中的 BM25 排序器（`k1=1.2`、`b=0.75`），实现 `RetrievalStrategy`。`new()` / `Default`。

### 类型 `VectorStrategy`
云端嵌入上的暴力余弦排序器，内置 `Bm25Strategy` 作为透明降级目标。详见下方「向量策略」小节。

### 类型 `HybridStrategy`
RRF（k=60）融合内置 `Bm25Strategy` + `VectorStrategy` 的两份全深度排名；详见 L3 与 L4：[retrieval/hybrid.rs](../L4-api/retrieval-hybrid.md)。

### 类型 `SubagentStrategy`
BM25 预筛出候选 shortlist，再交由注入的 `ChatModel`（小模型）重排（retrieve-then-rerank）。`new(chat, candidates)`
（`candidates` 经 `.max(1)` 夹紧）；常量 `DEFAULT_CANDIDATES = 20`。空 shortlist 直接返回空（不调 chat）；chat/解析
失败或选不出合法工具时**透明降级**到 BM25 shortlist；选中名赋合成递减分后取 `top_k`。prompt 构造与响应解析（白名单
去重保序、剔除幻觉）均为纯逻辑、可经 `MockChatModel` 测试。详见 L3 与 L4：[retrieval/subagent.rs](../L4-api/retrieval-subagent.md)。

### 错误 `StrategyError`
`enum StrategyError`（`thiserror` 派生），三个变体：

- `NotImplemented(String)`：未知/未实现的策略名。
- `EmbedderRequired(String)`：策略需要嵌入器但未提供（`"vector"`/`"hybrid"` 且 `backends.embedder` 为 `None`）。
- `ChatModelRequired(String)`：策略需要 chat 模型但未提供（`"subagent"` 且 `backends.chat` 为 `None`）。

### 类型 `Backends`
`#[derive(Default, Clone)] struct Backends { embedder: Option<Arc<dyn Embedder>>, chat: Option<Arc<dyn ChatModel>>,
subagent_candidates: Option<usize> }`。注入 `build_strategy` 的可选检索后端打包成一个结构体，让 factory 签名在新增
后端时保持稳定：`bm25` 无需任何后端，`vector`/`hybrid` 需 `embedder`，`subagent` 需 `chat`（并可选 `subagent_candidates`
覆盖 BM25 预筛大小）。以 `Option<Arc<...>>` 注入，使本 crate 仍不引入任何 HTTP 依赖。

### 工厂 `build_strategy`
`pub fn build_strategy(name: &str, backends: &Backends) -> Result<Box<dyn RetrievalStrategy>, StrategyError>`。

- `"bm25"` → `Bm25Strategy`，**无需**任何后端。
- `"vector"` → `VectorStrategy`，**要求** `backends.embedder`，否则返回 `EmbedderRequired`。
- `"hybrid"` → `HybridStrategy`，**要求** `backends.embedder`，否则返回 `EmbedderRequired`。
- `"subagent"` → `SubagentStrategy`，**要求** `backends.chat`，否则返回 `ChatModelRequired`；shortlist 大小取
  `subagent_candidates.unwrap_or(DEFAULT_CANDIDATES)`。未知名 → `NotImplemented`。

**接受 `&str` 而非配置类型**，使本 crate 不依赖 `config`（调用方传 `cfg.retrieval.strategy.as_str()`）。
后端以 `&Backends` 注入，使本 crate 仍不引入任何 HTTP 依赖。

> `strategy = "vector"`/`"hybrid"`/`"subagent"` 仅在 `serve`（在线网关，注入对应后端）下生效；离线的
> `search`/`get-details` CLI **不注入任何后端**，故 `build_strategy("vector", &Backends::default())` 返回
> `EmbedderRequired`、`build_strategy("subagent", &Backends::default())` 返回 `ChatModelRequired`。

### Embedder 抽象（HTTP 实现在独立 embedder crate）

把文本转成向量的可插拔抽象。本 crate **只**定义 trait、错误与确定性 mock；
**真实的 HTTP 后端 `OpenAiEmbedder` 位于独立的 `embedder` crate**，因此 `retrieval`
**不引入任何 HTTP 依赖**。

- trait `Embedder: Send + Sync`（`async_trait`）：
  - `async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>`：
    一批文本各转一个向量，顺序对应；每次调用 all-or-nothing。
  - `fn dim(&self) -> usize`：期望嵌入维度。
- 错误 `EmbedError`（`thiserror`，**provider 无关**）：
  `Provider(String)` 与 `Dimension { expected, got }`。
- `MockEmbedder`（**仅 `testkit` feature**）：确定性测试嵌入器。token 经 `tokenize` 后用
  FNV-1a 哈希分桶（`dim` 桶），共享 token 的文本余弦更高，便于断言排序。
  暴露 `calls`（调用计数）与 `seen`（已嵌文本）供后续缓存测试断言。
- `CachingEmbedder`（**始终可用**，非 testkit 门控）：`Embedder` 装饰器，包装任意 `Arc<dyn Embedder>`，
  **以文本 `String` 为键记忆**向量（键即文本，碰撞在结构上不可能）；只把缓存未命中的文本转发给内层，保序、维度不变。`dim()` 透传内层。
  缓存为**两代有界缓存**（`current`+`previous`，各上限 `CACHE_GEN_CAP = 2048`，内存约 `2*CAP`，promote-on-hit），
  在 `mcpgw` 中**只构造一次**，使缓存跨快照重建持续存在（`list_changed` 时只嵌入新增工具文本）。

逐文件 API 见 L4：[retrieval/embedder.rs](../L4-api/retrieval-embedder.md)。

### 抽象 ChatModel（HTTP 实现在独立 chat crate）

单轮 chat 补全的可插拔抽象（system + user → 文本）。本 crate **只**定义 trait、错误与确定性 mock；
**真实的 HTTP 后端 `OpenAiChat` 位于独立的 `chat` crate**，因此 `retrieval` **不引入任何 HTTP 依赖**。

- trait `ChatModel: Send + Sync`（`async_trait`）：
  - `async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError>`：一次系统 + 用户 prompt
    产生助手文本。
- 错误 `ChatError`（`thiserror`，**provider 无关**）：`Provider(String)` 与 `Empty`（无可用内容）。
- `MockChatModel`（**仅 `testkit` feature**）：确定性测试 chat 模型，返回脚本化 `reply`（或 `failing()` 时报错），
  记录调用计数与最后一次 (system, user) prompt，供 `SubagentStrategy` 重排测试断言。

由 `SubagentStrategy` 用于重排候选工具。逐文件实现见 L4：[chat/lib.rs](../L4-api/chat-openai.md)；组件见 L2：[chat](./chat.md)。

### 向量策略 `VectorStrategy`

`VectorStrategy::new(embedder: Arc<dyn Embedder>)`：持有一个 `embedder`、一个内置 `Bm25Strategy` 与一个
`degraded` 标志，在云端嵌入上做**暴力余弦检索**（目录小，归一化后线性扫描，cosine == 点积）。

- **每工具嵌入文本**为 `"{qualified_name}\n{description}"`（换行分隔）。
- `index` **总是先（重）建 BM25**，再尝试批量嵌入全部工具文本；嵌入失败 → `tracing::warn!`、清空向量、
  置 `degraded = true`（索引期降级）。
- `search` 在以下任一情况返回 **BM25** 结果（透明降级）：已 `degraded`、无向量、或**本次 query 嵌入失败**
  （仅本次回退，不改 `degraded`）；否则对归一化向量做暴力余弦，按"分数降序 + qualified_name 升序"排序后取
  `top_k`。零范数向量被守卫（cosine 记 0），**不产生 NaN**。

> 内部细节（双降级路径、锁纪律、排序不变量）见 L3：[retrieval](../L3-details/retrieval.md)。

## 依赖

- 外部：`thiserror`、`async-trait`、`tracing`、`serde_json`（解析 subagent 的 LLM JSON 回复）；（dev）`tokio`。
- 内部：`catalog`。**不依赖 `config`，也不引入任何 HTTP 依赖**（HTTP 后端隔离在 `embedder` / `chat` crate）。

## 被谁使用

- `mcpgw`：`build_strategy(cfg.retrieval.strategy.as_str(), &backends)` → `index` → `search`；向量/混合路径下
  `backends.embedder` 注入由 `embedder` crate 构造、再被 `CachingEmbedder` 包装的 `Arc<dyn Embedder>`；subagent
  路径下 `backends.chat` 注入由 `chat` crate 构造的 `Arc<dyn ChatModel>`（外加 `subagent_candidates`）。

## 关键不变量

- `idf()` 恒为正（见 L3），故 `search` 中 `score > 0.0` 等价于"至少命中一个查询词"。
- 排序为"分数降序 + qualified_name 升序"做 tie-break → 完全确定（golden 依赖）。

## 向下导航

- 内部细节（BM25 算法、向量策略、嵌入缓存）见 L3：[retrieval](../L3-details/retrieval.md)
- 逐文件 API 见 L4：[retrieval/lib.rs](../L4-api/retrieval-lib.md) ·
  [retrieval/embedder.rs](../L4-api/retrieval-embedder.md) · [retrieval/vector.rs](../L4-api/retrieval-vector.md) ·
  [retrieval/hybrid.rs](../L4-api/retrieval-hybrid.md) · [retrieval/subagent.rs](../L4-api/retrieval-subagent.md)
- 真实 HTTP 后端见姊妹组件：[embedder](./embedder.md) · [chat](./chat.md)
