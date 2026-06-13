"""Part 4 (next steps) — Hybrid retrieval placeholder (awaiting M2-B)."""
import placeholder

LESSON_15 = placeholder.build(
    "Hybrid 检索用 RRF（Reciprocal Rank Fusion）融合 BM25 与向量两路排名，兼顾字面精确与语义召回。该能力属于 M2-B，尚未实现，本课待其落地后补全。",
    [
        "RRF 融合公式与为何优于单路（字面 vs 语义互补）",
        "<code>build_strategy(\"hybrid\", …)</code> 目前返回 <code>StrategyError::NotImplemented</code>，M2-B 将实现",
        "默认检索策略从 bm25 切换到 hybrid 的考量",
    ],
    [
        ("04-vector-overview.html", "04 · 向量检索总览（可插拔策略 + 透明降级）"),
        ("07-vector-strategy.html", "07 · VectorStrategy（向量一路）"),
        ("13-retrieval-bm25.html", "13 · retrieval 内核与 BM25（字面一路）"),
    ],
)
