# L2 — `retrieval` 组件

## 职责

工具**排序/检索**。定义可插拔的 `RetrievalStrategy` 抽象，并提供 M0 的默认实现：自研 BM25。
只了解 `catalog` 的类型；不了解配置文件或 CLI。

## 公开接口

### 类型 `ScoredTool`
一次检索命中：`{ qualified_name: String, description: String, score: f32 }`。派生 `Debug, Clone, PartialEq`
（**不含 `Eq`/`Hash`**，因为 `score: f32` 不可全序/不可哈希）。

### trait `RetrievalStrategy: Send + Sync`
可插拔策略抽象。

| 方法 | 签名 | 说明 |
|------|------|------|
| `index` | `(&mut self, &Catalog)` | 从当前目录（重）建内部索引 |
| `search` | `(&self, query: &str, top_k: usize) -> Vec<ScoredTool>` | 返回最多 `top_k` 条，按相关性降序 |

### 函数 `tokenize`
`pub fn tokenize(text: &str) -> Vec<String>`：小写化、按非字母数字边界切分（`_` 也作为边界）、丢空串。
Unicode 感知（`char::is_alphanumeric` + `to_lowercase`）。

### 类型 `Bm25Strategy`
内存中的 BM25 排序器（`k1=1.2`、`b=0.75`），实现 `RetrievalStrategy`。`new()` / `Default`。

### 错误 `StrategyError`
`enum StrategyError { NotImplemented(String) }`（`thiserror` 派生）。

### 工厂 `build_strategy`
`pub fn build_strategy(strategy: &str) -> Result<Box<dyn RetrievalStrategy>, StrategyError>`。
M0 仅实现 `"bm25"`；`"vector"`/`"hybrid"`/未知 → `NotImplemented`。
**接受 `&str` 而非配置类型**，使本 crate 不依赖 `config`。

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

逐文件 API 见 L4：[retrieval/embedder.rs](../L4-api/retrieval-embedder.md)。

## 依赖

- 外部：`thiserror`；（dev）`serde_json`。
- 内部：`catalog`。**不依赖 `config`。**

## 被谁使用

- `mcpgw`：`build_strategy(cfg.retrieval.strategy.as_str())` → `index` → `search`。

## 关键不变量

- `idf()` 恒为正（见 L3），故 `search` 中 `score > 0.0` 等价于"至少命中一个查询词"。
- 排序为"分数降序 + qualified_name 升序"做 tie-break → 完全确定（golden 依赖）。

## 向下导航

- 内部细节（BM25 算法）见 L3：[retrieval](../L3-details/retrieval.md)
- 逐文件 API 见 L4：[retrieval/lib.rs](../L4-api/retrieval-lib.md)
