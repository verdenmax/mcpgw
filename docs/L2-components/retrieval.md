# L2 — `retrieval` 组件

## 职责

工具**排序/检索**。定义可插拔、**异步**（`#[async_trait]`）的 `RetrievalStrategy` 抽象，并提供两种实现：
M0 的默认实现自研 **BM25**（`Bm25Strategy`），以及 M2-A 新增的 **`VectorStrategy`**（云端嵌入余弦检索，
内置 BM25 透明降级）。同时定义 `Embedder` 抽象与 `CachingEmbedder` 装饰器（真实 HTTP 后端在独立
`embedder` crate）。只了解 `catalog` 的类型；不了解配置文件或 CLI。

> **默认策略仍是 `bm25`**：仅当配置 `strategy = "vector"` 且提供 `[retrieval.vector]` 时才走向量路径。

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

### 错误 `StrategyError`
`enum StrategyError`（`thiserror` 派生），两个变体：

- `NotImplemented(String)`：未实现的策略名（`"hybrid"`（延后到 M2-B）、未知名）。
- `EmbedderRequired(String)`：策略需要嵌入器但未提供（`"vector"` 且 `embedder` 为 `None`）。

### 工厂 `build_strategy`
`pub fn build_strategy(name: &str, embedder: Option<&Arc<dyn Embedder>>) -> Result<Box<dyn RetrievalStrategy>, StrategyError>`。

- `"bm25"` → `Bm25Strategy`，**无需** embedder。
- `"vector"` → `VectorStrategy`，**要求** embedder，否则返回 `EmbedderRequired`。
- `"hybrid"` → `NotImplemented`（延后到 M2-B）；未知名 → `NotImplemented`。

**接受 `&str` 而非配置类型**，使本 crate 不依赖 `config`（调用方传 `cfg.retrieval.strategy.as_str()`）。
embedder 以 `Option<&Arc<dyn Embedder>>` 注入，使本 crate 仍不引入任何 HTTP 依赖。

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
  **按文本内容哈希（FNV-1a）记忆**向量；只把缓存未命中的文本转发给内层，保序、维度不变。`dim()` 透传内层。
  在 `mcpgw` 中**只构造一次**，使缓存跨快照重建持续存在（`list_changed` 时只嵌入新增工具文本）。

逐文件 API 见 L4：[retrieval/embedder.rs](../L4-api/retrieval-embedder.md)。

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

- 外部：`thiserror`、`async-trait`、`tracing`；（dev）`serde_json`、`tokio`。
- 内部：`catalog`。**不依赖 `config`，也不引入任何 HTTP 依赖**（HTTP 后端隔离在 `embedder` crate）。

## 被谁使用

- `mcpgw`：`build_strategy(cfg.retrieval.strategy.as_str(), embedder.as_ref())` → `index` → `search`；
  向量路径下注入由 `embedder` crate 构造、再被 `CachingEmbedder` 包装的 `Arc<dyn Embedder>`。

## 关键不变量

- `idf()` 恒为正（见 L3），故 `search` 中 `score > 0.0` 等价于"至少命中一个查询词"。
- 排序为"分数降序 + qualified_name 升序"做 tie-break → 完全确定（golden 依赖）。

## 向下导航

- 内部细节（BM25 算法、向量策略、嵌入缓存）见 L3：[retrieval](../L3-details/retrieval.md)
- 逐文件 API 见 L4：[retrieval/lib.rs](../L4-api/retrieval-lib.md) ·
  [retrieval/embedder.rs](../L4-api/retrieval-embedder.md) · [retrieval/vector.rs](../L4-api/retrieval-vector.md)
- 真实 HTTP 嵌入后端见姊妹组件：[embedder](./embedder.md)
