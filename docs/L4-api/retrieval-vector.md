# L4 — `crates/retrieval/src/vector.rs` API

源文件：`crates/retrieval/src/vector.rs`。

`VectorStrategy`：对云端嵌入做**暴力余弦检索**，并内置一个 `Bm25Strategy` 作为**透明降级**目标
（索引期批量嵌入失败，或某次查询嵌入失败时）。工具目录很小，归一化向量上的线性扫描（余弦 == 点积）
完全够用，因此**不引入 ANN 索引**（YAGNI）。

## `struct VectorStrategy`
```rust
pub struct VectorStrategy {
    embedder: Arc<dyn Embedder>,
    bm25: Bm25Strategy,
    /// (qualified_name, description, normalized embedding) — empty when degraded.
    vectors: Vec<(String, String, Vec<f32>)>,
    degraded: bool,
}
```

- `embedder`：注入的 `Arc<dyn Embedder>`，索引/查询两期均通过它嵌入文本。
- `bm25`：内置的 BM25 降级策略，索引期总是先（重）建。
- `vectors`：每个工具的 `(qualified_name, description, L2 归一化向量)`；降级时为空。
- `degraded`：索引期嵌入失败时置 `true`，查询期直接走 BM25。

## `VectorStrategy::new`
```rust
pub fn new(embedder: Arc<dyn Embedder>) -> Self
```
构造一个空策略：内置 `Bm25Strategy::new()`、空 `vectors`、`degraded = false`。需调用 `index` 后才可检索。

## `impl RetrievalStrategy for VectorStrategy`

### `index(&mut self, catalog: &Catalog)`
1. **总是先（重）建 BM25 降级**：`self.bm25 = Bm25Strategy::new()` 后 `bm25.index(catalog).await`，
   保证即使后续嵌入失败也有可用的回退索引。
2. 收集所有工具，对每个工具构造嵌入文本 `tool_text` = `"{qualified_name}\n{description}"`。
3. 调用 `embedder.embed(&texts)`：
   - **成功**：把每个返回向量 `normalize` 后存入 `vectors`，`degraded = false`。
   - **失败**：`tracing::warn!` 记录错误，清空 `vectors`，置 `degraded = true`。

### `search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>`
1. **降级判定**：若 `degraded` 或 `vectors` 为空 → 直接返回 `bm25.search(query, top_k).await`。
2. 否则嵌入查询：`embedder.embed(&[query])`：
   - **成功**：取首向量并 `normalize`。
   - **失败**：`tracing::warn!` 后回退 `bm25.search`（**per-query 降级**，不改变 `degraded` 状态）。
3. 对每个工具向量计算 `dot(qv, v)`（归一化后即余弦），构造 `ScoredTool`。
4. `sort_by`：分数**降序**，同分按 `qualified_name` **升序**（确定性 tie-break），最后 `truncate(top_k)`。

## 私有辅助

### `fn normalize(mut v: Vec<f32>) -> Vec<f32>`
L2 归一化（原地）。**零范数保护**：`if norm > 0.0` 才除，零向量原样保留——否则会得到 `NaN`。
零向量与任何向量的余弦/点积因此为 `0`，而非 `NaN`。这条不变量必须保留（T2 review 指出嵌入器对无 token
文本可能返回零向量）。

### `fn dot(a: &[f32], b: &[f32]) -> f32`
逐元素相乘求和。两向量均已 L2 归一化时，点积即余弦相似度。

### `fn tool_text(t: &ToolDef) -> String`
每个工具的嵌入文本：`"{qualified_name}\n{description}"`。

## 相关

- 嵌入抽象见 L4：[retrieval/embedder.rs](./retrieval-embedder.md)；BM25 细节见 L3：[retrieval](../L3-details/retrieval.md)。
