# L4 — `crates/retrieval/src/hybrid.rs` API

源文件：`crates/retrieval/src/hybrid.rs`。

## `struct HybridStrategy`
```rust
pub struct HybridStrategy { /* bm25, vector, doc_count（均私有） */ }

impl HybridStrategy {
    pub fn new(embedder: std::sync::Arc<dyn Embedder>) -> Self;
}
```
RRF 混合检索器：内部组合一个 `Bm25Strategy` 与一个 `VectorStrategy`（后者持有 `embedder`、自带 BM25 降级）。
实现 `RetrievalStrategy`（async）。一般经 `build_strategy("hybrid", Some(embedder))` 构造。

- `index(&mut self, &Catalog)`：分别 `index` BM25 与向量分量，并记录 `doc_count`（= 目录工具数，用作全深度子检索的 `top_k`）。
- `search(&self, query, top_k)`：`doc_count == 0` → 空；否则取 `bm25.search(query, doc_count)` 与
  `vector.search(query, doc_count)` 两份**全深度**子排名，做 RRF 融合后取 `top_k`。

## 私有 `fn rrf_fuse` / 常量 `RRF_K`
- `const RRF_K: f32 = 60.0`（业界标准，**不暴露配置**）。
- `rrf_fuse(lists, top_k)`：对每份列表按名次 `rank`（从 1）累加 `1/(RRF_K + rank)`，按 `qualified_name`
  求和；按「融合分降序 + `qualified_name` 升序」排序后截到 `top_k`。`ScoredTool.score` 承载**融合分**
  （量级小，例如双表 rank1 ≈ `2/61`），**不可跨策略比较**。私有，不属公开 API。

## 行为要点
- BM25 子表只含命中词项的文档；向量子表含全部文档 → 仅语义相关者仍可经向量表进入融合（召回增益）。
- embedding 失败时向量分量返回内部 BM25 排名 → 两份子表≈同一 BM25 排名 → 融合后退化≈纯 BM25（自愈，无需额外标志）。

> 算法/数据流见 L3：[retrieval](../L3-details/retrieval.md)。
