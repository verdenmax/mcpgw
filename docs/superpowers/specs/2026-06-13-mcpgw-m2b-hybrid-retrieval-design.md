# M2-B 设计：混合检索（RRF 融合 BM25 + 向量）

> 状态：已通过 brainstorm 评审，待 writing-plans 细化为实现计划。
> 前置：M0 / M1（A/B.1/B.2/C）/ M2-A 已合并到 master（HEAD `a94dc44`）。
> 关联里程碑：roadmap `M2`（检索深度）的 M2-B（紧接 M2-A 向量基础）。

## 1. 目标与范围

在 M2-A 已落地的「异步、可插拔、失败可降级」检索架构之上，新增 **`HybridStrategy`**：用
**Reciprocal Rank Fusion（RRF）** 把 BM25 的词法排名与向量的语义排名融合，兼得「关键词精确」与
「语义召回」。

**范围内：**
- 新增 `HybridStrategy`：组合现有 `Bm25Strategy` + `VectorStrategy`，对两份排名做 RRF 融合。
- `build_strategy` 新增 `"hybrid"` 臂：需要 `embedder`（与 `"vector"` 一致，无则 `EmbedderRequired`）。
- `config` 校验：`strategy = "hybrid"` 也要求 `[retrieval.vector]`（复用同一段，无新增配置段）。
- `mcpgw` 启动期：`build_embedder` 对 `"hybrid"` 也构建 embedder 并注入 `GatewayState`。
- RRF 常数 **`k = 60` 固定**（不暴露配置旋钮，YAGNI）。

**明确不含（留后续）：**
- **不切换默认策略**：默认仍为 `bm25`（零依赖、离线可用）。`vector`/`hybrid` 均经 config 显式开启。
- 可视化指南（mcpgw-visual-guide）的混合章节（09–15 占位）——作为独立后续任务，本 spec 不含。
- `SubagentStrategy`（M2.T5）、本地离线 embedding（M2.T7）、RRF 加权/可调 `k`、候选集截断窗口等。

## 2. 决策记录（brainstorm 已敲定）

| 决策点 | 结论 | 理由 |
|--------|------|------|
| 默认策略是否改 | **否，仍 `bm25`** | hybrid 需 embedder（云 API + key），不能做真正零配置默认；保住离线开箱即用 |
| hybrid 是否需 `[retrieval.vector]` | **是（与 vector 一致）** | hybrid 必含向量分量，必须有 embedder |
| RRF `k` | **固定 60** | 业界标准默认；不加配置面，保持最小 surface |
| HybridStrategy 内部构成 | **组合 `Bm25Strategy` + `VectorStrategy`** | 最大化复用 M2-A；`VectorStrategy` 自带 BM25 降级，融合天然自愈 |

## 3. crate 划分（仅 retrieval 新增组件 + 两处接线）

| crate | 新增/改动 |
|------|-----------|
| `retrieval` | **新增 `HybridStrategy`**（`src/hybrid.rs`）；`build_strategy` 加 `"hybrid"` 臂；`lib.rs` re-export `HybridStrategy` |
| `config` | `validate`：把「strategy=="vector" 才要求 `[retrieval.vector]`」放宽为 `matches!(strategy, "vector" \| "hybrid")` |
| `mcpgw` | `build_embedder`：匹配臂 `"vector"` → `"vector" \| "hybrid"`，让 hybrid 也建 embedder 并注入 |
| `metatools` / `gateway` / `downstream` | **无改动**：已对策略/embedder/async 泛化；`GatewayState` 已跨 rebuild 持有 embedder |

> 无新依赖：`HybridStrategy` 纯逻辑，复用 `Bm25Strategy`/`VectorStrategy`/`ScoredTool`。retrieval 仍不引入 HTTP。

## 4. 抽象与数据流

### 4.1 `HybridStrategy`（retrieval，async）

```rust
pub struct HybridStrategy {
    bm25: Bm25Strategy,
    vector: VectorStrategy,
    doc_count: usize,
}

impl HybridStrategy {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            bm25: Bm25Strategy::new(),
            vector: VectorStrategy::new(embedder),
            doc_count: 0,
        }
    }
}
```

- `index(catalog)`：分别 `self.bm25.index(catalog).await` 与 `self.vector.index(catalog).await`；
  记录 `self.doc_count = catalog.iter().count()`（用作全深度子检索的 `top_k`）。
- `search(query, top_k)`：
  1. **全深度子排名**：`lb = bm25.search(query, doc_count)`、`lv = vector.search(query, doc_count)`。
     用 `doc_count` 而非 `top_k` 是因为 RRF 必须看到每个文档在各列表中的**真实名次**；若先各自截到
     `top_k` 再融合，会丢掉「在一边名次低、另一边名次高」的单边命中。工具目录小，全深度可接受。
     `doc_count == 0` → 两表皆空 → 返回空。
  2. **RRF 融合**：对每份列表按名次 `rank`（从 1 起）累加贡献 `1.0 / (60.0 + rank)`，按
     `qualified_name` 求和进一个 map；同时记录该名字的 `description`（两表 description 一致，取先见者）。
  3. **排序与截断**：按融合分降序排序，并以 `qualified_name` 升序作为**稳定 tie-break**（确定性，
     与 `Bm25Strategy`/`VectorStrategy` 现有约定一致）；`truncate(top_k)`。
- `ScoredTool.score` 承载 **RRF 融合分**（量级很小，例如 rank=1 两表命中 ≈ `2/61 ≈ 0.0328`）。
  此分**不可跨策略比较**，仅用于本策略内排序——L3/L4 文档须注明。

### 4.2 RRF 公式

```
fused(doc) = Σ_{L ∈ {bm25, vector}}  [ doc ∈ L ]  ·  1 / (k + rank_L(doc)),   k = 60
```

- `rank_L(doc)`：doc 在列表 L 中的名次（最佳为 1）。doc 不在某列表则该项不计。
- 单调、确定、无需归一化即可融合两份异量纲排名——这正是 RRF 相对「分数线性加权」的优势。

## 5. 行为与边界情形

- **BM25 只产出命中词项的文档**（`score>0` 过滤）；**向量对全部文档打分**并排名。因此「仅语义相关、
  无词法命中」的工具仍能经向量列表进入融合（这是 hybrid 相对纯 BM25 的召回增益）。
- **降级天然自愈**：embedding 失败时（索引期或查询期），`VectorStrategy` 已会返回其内部 BM25 排名。
  此时两份列表≈相同 BM25 排名，RRF 融合后名次单调一致 → hybrid 退化≈纯 BM25。**无需额外 degraded 标志**。
- **空目录**（`doc_count == 0`）→ 空结果。`top_k > 目录大小` → `truncate` 无操作。
- **确定性**：给定 embedder 输出，结果完全确定，可用 `MockEmbedder` 做 golden。

## 6. 工厂与配置接线

### 6.1 `build_strategy`（`retrieval/src/lib.rs`）

```rust
"hybrid" => match embedder {
    Some(e) => Ok(Box::new(HybridStrategy::new(e.clone()))),
    None => Err(StrategyError::EmbedderRequired(name.to_string())),
},
```

- 与 `"vector"` 完全对称：缺 embedder → `EmbedderRequired`（不再是旧的 `NotImplemented`）。

### 6.2 `config::validate`

- 现状：仅 `strategy == "vector"` 时要求 `[retrieval.vector]`。
- 改为：`if matches!(self.retrieval.strategy.as_str(), "vector" | "hybrid")` 时执行同一组
  「`[retrieval.vector]` 必须存在且 base_url/model/api_key_env 非空」校验。
- **需同步更新现有测试** `parses_retrieval_section`（当前用 `strategy="hybrid"` 且无 vector 段、期望 Ok）：
  改为补 `[retrieval.vector]` 段使其仍合法，或断言其在新规则下报 `Invalid`（择一，由实现计划定）。
- `KNOWN` 白名单已含 `"hybrid"`，无需改。

### 6.3 `mcpgw::build_embedder`

- 匹配臂 `"vector" => { 读 [retrieval.vector]、读 key、建 OpenAiEmbedder + CachingEmbedder }`
  扩为 `"vector" | "hybrid" => { 同上 }`。其余启动流程（`with_embedder(strategy, embedder)`）不变。

## 7. 测试策略（含 corner case；专门测试可由子代理补）

- **`hybrid.rs` 单元**：
  - RRF 数学与 tie-break（构造已知名次，验证融合分与排序、同分按 `qualified_name` 升序）。
  - **全深度融合**：某工具在 BM25 名次低、却被向量列表高位召回，验证 hybrid 将其排到合理位置
    （证明未在融合前被 `top_k` 截掉）；`MockEmbedder::new`。
  - **降级路径**：`MockEmbedder::failing` → hybrid 排序与纯 BM25 一致。
  - 空目录 → 空；`top_k` 截断；排序稳定性。
- **golden（`MockEmbedder`）**：hybrid 把「词法 + 语义」双命中的工具排第一；与 bm25-only / vector-only
  对照，展示融合差异（仅语义命中的工具被 hybrid 召回）。
- **`build_strategy`**：`hybrid`+None → `EmbedderRequired`；`hybrid`+Some → Ok 且可 index/search。
- **`config`**：新增「`strategy="hybrid"` 缺 `[retrieval.vector]` → `Invalid`」；更新 `parses_retrieval_section`。
- **`gateway`**：`with_embedder("hybrid", MockEmbedder)` rebuild 能建出 hybrid 快照（镜像现有 vector 用例）。
- **`mcpgw`**：`build_embedder` 对 `"hybrid"` 也能建 embedder（镜像现有 vector 用例）。
- **可选**：门控真实语义冒烟（`#[ignore]` / testkit），镜像 M2-A 的向量冒烟。

## 8. 分层文档（Definition of Done，随代码同提交）

- **L4**：新增 `docs/L4-api/retrieval-hybrid.md`（`HybridStrategy`）；更新 `retrieval-lib.md`
  （`build_strategy` 的 hybrid 臂）、`config-lib.md`（validate 放宽）、`mcpgw-main.md`（build_embedder）。
- **L3**：`retrieval.md`——RRF 算法、全深度融合、降级自愈、分数语义。
- **L2**：`retrieval.md`（hybrid 已实现）、`config.md` / `mcpgw-cli.md`（按需）。
- **L1**：`L1-overview.md`——默认仍 bm25；hybrid 可用；M2-B 完成。
- **索引**：`docs/README.md` 的 L4 清单加 `retrieval-hybrid.md`，里程碑覆盖说明加 M2-B。
- **路线图**：`docs/superpowers/plans/2026-06-08-mcpgw-program-roadmap.md` 标 M2-B 完成。

## 9. 实现期需现场确认/可能回退的点

- `bm25.search` 的 `score>0` 过滤使 BM25 列表只含命中词项的文档——RRF 下这是预期（未命中者仅靠向量列表
  获得贡献）。需在测试中固化该不对称行为。
- 含 embedding 时 `VectorStrategy` 对**全部**文档排名，故 hybrid 对任意非空 query 总能返回最多 `top_k` 个
  「语义最近」结果（即便无词法命中）——这与纯 BM25「无命中即空」不同，属语义召回的预期行为，文档须注明。
- `doc_count` 取 `catalog.iter().count()`；确认与 `VectorStrategy`/`Bm25Strategy` 实际索引的文档数一致
  （二者均逐 `catalog.iter()` 建索引，无过滤），避免全深度子检索深度不足。
- 现有 `build_strategy_errors_appropriately` 测试断言 `build_strategy("hybrid", None)` 为 `NotImplemented`，
  M2-B 后应改为 `EmbedderRequired`——实现计划须更新该断言。
