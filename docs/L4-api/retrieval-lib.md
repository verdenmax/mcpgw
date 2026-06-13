# L4 — `crates/retrieval/src/lib.rs` API

源文件：`crates/retrieval/src/lib.rs`。

## `struct ScoredTool`
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredTool {
    pub qualified_name: String,
    pub description: String,
    pub score: f32,
}
```
一次检索命中。不派生 `Eq`/`Hash`（因 `f32`）。

## `trait RetrievalStrategy`
```rust
#[async_trait]
pub trait RetrievalStrategy: Send + Sync {
    async fn index(&mut self, catalog: &catalog::Catalog);
    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}
```
可插拔检索策略抽象（`#[async_trait]`，两方法均为 `async`，保持 `Box<dyn RetrievalStrategy>` 对象安全）。
`index` 从目录（重）建索引；`search` 返回最多 `top_k` 条、相关性降序。

## `fn tokenize`
```rust
pub fn tokenize(text: &str) -> Vec<String>
```
小写化、按非字母数字边界（含 `_`）切分、丢空串。Unicode 感知。

## `struct Bm25Strategy`
```rust
#[derive(Debug, Clone)]
pub struct Bm25Strategy { /* k1,b,docs,doc_freq,avgdl,n（均私有） */ }
```
内存 BM25 排序器（`k1=1.2`、`b=0.75`），实现 `RetrievalStrategy`。
- `pub fn new() -> Self`
- 实现 `Default`（= `new()`）。
- 算法细节见 L3：[retrieval](../L3-details/retrieval.md)。

## `enum StrategyError`
```rust
#[derive(Debug, thiserror::Error)]
pub enum StrategyError {
    #[error("retrieval strategy {0:?} is not implemented in this version")]
    NotImplemented(String),
    #[error("retrieval strategy {0:?} requires an embedder but none was configured")]
    EmbedderRequired(String),
}
```
- `NotImplemented(String)`：未知/未实现的策略名。
- `EmbedderRequired(String)`：策略需要 embedder 但未提供（`"vector"`/`"hybrid"` 且 `embedder` 为 `None`）。

## `fn build_strategy`
```rust
pub fn build_strategy(
    name: &str,
    embedder: Option<&std::sync::Arc<dyn Embedder>>,
) -> Result<Box<dyn RetrievalStrategy>, StrategyError>
```
按名构造策略：
- `"bm25"` → `Bm25Strategy`（无需 embedder）。
- `"vector"` → `VectorStrategy`（要求 embedder，否则 `EmbedderRequired`）。
- `"hybrid"` → `HybridStrategy`（要求 embedder，否则 `EmbedderRequired`）。
- 其余 → `NotImplemented`。

**接受 `&str`（而非 config 类型），使本 crate 不依赖 `config`。**

> 内部数据结构 `IndexedDoc` 为私有，不属于公开 API。

> 逐文件 hybrid API 见 [retrieval/hybrid.rs](./retrieval-hybrid.md)。
