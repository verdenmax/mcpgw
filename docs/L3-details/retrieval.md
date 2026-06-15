# L3 — `retrieval` 细节（BM25 / 向量 / 混合 / subagent）

## 分词 `tokenize`

规则：按"非字母数字"边界切分（`char::is_alphanumeric` 的反面）→ 丢空串 → `to_lowercase()`。

- 因为 `_` 不是字母数字，所以 `_` 也是切分边界：`github__create_issue` → `["github","create","issue"]`。
- **Unicode 感知**：非 ASCII 文字与大小写映射开箱即用（非 `is_ascii_*` 陷阱）。
- **已知限制**：不做 Unicode 归一化（`café` ≠ `cafe`）；不拆 camelCase（`createIssue` 为单 token）。
  对以 ASCII 为主的工具描述足够；若改为 ASCII 优化需同步更新 golden。

## 索引构建 `Bm25Strategy::index`

对每个工具，"可检索文本" = `qualified_name() + " " + description`，分词后：

- `term_freq`：该文档内每个词的词频（per-doc TF）。
- `doc_freq`：语料级文档频率，**每个不同词在一篇文档内只 +1**（遍历 `term_freq.keys()`，而非原始 token
  序列）—— 这是 DF 的正确定义。
- `avgdl`：平均文档长度（`total_len / n`）；`n == 0` 时置 0。
- 结果整体替换 `self.{docs, doc_freq, avgdl, n}`。

## 评分 `Bm25Strategy::search`

对每篇文档，累加查询中"命中词"的 BM25 贡献：

```
score += idf(t) · ( f·(k1+1) ) / ( f + k1·(1 − b + b·(len/avgdl)) )
```

其中 `f` = 词 t 在该文档的 TF，`len` = 该文档长度，`k1 = 1.2`，`b = 0.75`。

### IDF（Lucene / BM25⁺ 变体，恒正）

```
idf(t) = ln( 1 + (N − df + 0.5) / (df + 0.5) )
```

`+ 1` 保证 ln 的参数 `> 1`，从而 **idf 恒 > 0**。这对小语料至关重要：单文档（`df = N = 1`）时
`ln(1 + 0.5/1.5) ≈ 0.287 > 0`；若用教科书式 IDF 会得到负值。

### `score > 0.0` 过滤的不变量耦合

`search` 用 `score > 0.0` 作为"是否至少命中一个查询词"的判据。**这只在 idf 恒正时成立**：命中词贡献严格
为正，未命中文档保持初始 `0.0`。代码注释明确锁定了这条不变量——若将来改用可能为负的 IDF，必须同步修改
此过滤，否则会丢弃合法命中。

### 排序与 tie-break

`sort_by`：分数**降序**，同分时按 `qualified_name` **升序**。由于 qualified name 唯一，排序完全确定
（不依赖排序稳定性），这是 golden 测试可复现的基础。最后 `truncate(top_k)`。

### 边界与数值

- 空索引或 `avgdl == 0.0` → 直接返回空（避免除零）。`avgdl == 0.0` 的精确浮点比较安全，因为它是显式
  赋的字面量而非计算后比较。
- `denom` 运行期不会为 0：命中要求 `f ≥ 1`，故 `denom ≥ 1`。
- 所有分数有限 → `partial_cmp` 不会返回 `None`；`unwrap_or(Equal)` 为防御性兜底。

## 复杂度 / 为何自研而非 tantivy

- 目录小（数十~数百工具），`index` 与 `search` 均为线性扫描，开销可忽略。
- 自研 BM25：确定性、零索引生命周期/提交复杂度、无外部 API 漂移风险，且便于 golden 测试。
- `tantivy` 是 catalog 规模增大后的升级路径——在同一 `RetrievalStrategy` trait 后可直接替换。

## 工厂与解耦

`build_strategy(name: &str, backends: &Backends)` 按名构造策略：`"bm25"` → `Bm25Strategy`（无需后端）；
`"vector"`/`"hybrid"` → 需 `backends.embedder`（否则 `EmbedderRequired`）；`"subagent"` → 需 `backends.chat`
（否则 `ChatModelRequired`）；其余 → `StrategyError::NotImplemented`。**接受 `&str` 而非 `RetrievalConfig`**：
使 `retrieval` 不依赖 `config`，符合"核心排序 crate 仅依赖 `catalog`"的边界（由 mcpgw 传入
`cfg.retrieval.strategy.as_str()`）。可选后端打包进 `Backends { embedder, chat, subagent_candidates }`，
让 factory 签名在新增后端时保持稳定。

## 测试覆盖

- `tokenize_splits_on_non_alphanumeric_and_lowercases`
- `bm25_ranks_relevant_tool_first` / `bm25_respects_top_k_and_filters_zero_score`
- `build_strategy_returns_bm25_and_indexes` / `build_strategy_errors_on_unimplemented_strategies`
- 集成 golden 测试：`crates/retrieval/tests/golden.rs`（4 个查询的 top-1 期望，3 个为真正的排序判别）。

## 异步化（M2-A T1）

为支持 M2-A 的云端向量嵌入（索引/检索期需异步网络调用），`RetrievalStrategy` trait 已整体异步化：

- 借助 `async-trait`（`#[async_trait]`）把 `index`/`search` 改为 `async fn`，同时保持 `Box<dyn RetrievalStrategy>`
  对象安全；`async-trait` 默认把方法 future 装箱为 `Send`，可继续跨 `.await` 持有 `Arc<GatewaySnapshot>`。
- `Bm25Strategy` 的方法体逐字不变，仅签名加 `async`——本次为纯机械重构，不新增任何检索功能。
- `GatewayState::new` 不再在构造期 `index`（构造函数保持同步）：空目录检索结果本就为空，**首个真实快照由异步
  `rebuild_snapshot` 建立**。
- 全链路 `await`：`metatools::search_tools` → `strategy.search` 全部 `.await`；`downstream` 的 `search_tools`
  臂、`mcpgw` CLI 的 `search` 子命令（用 current-thread runtime `block_on`）均已贯通。

## 嵌入缓存（M2-A T3）

`CachingEmbedder`（`crates/retrieval/src/caching.rs`）是 `Embedder` 装饰器，包装任意 `Arc<dyn Embedder>`：

- **缓存键 = 文本内容哈希**（FNV-1a）：相同文本内容映射到同一缓存项，与位置无关。
- **跨 rebuild 复用**：后续 `GatewayState` 持有 `Arc<CachingEmbedder>`，缓存在多次快照重建间持续存在；
  **`list_changed` 时只对新增工具文本调用内层 embedder**，未变工具直接命中缓存。
- **仅嵌未命中 + 保序**：单次 `embed` 内先按哈希去重收集未命中文本（首见顺序），只把这些转发给内层，
  再按原始输入顺序还原向量；批内重复文本只嵌一次。还原读的是调用内**本地 `resolved` map**（非有界缓存本身），
  故即便单批超大、嵌入途中触发缓存轮转/驱逐，每个输入仍拿到正确向量。
- **全命中跳过内层**：整批命中时完全不发起内层调用，省去网络往返。
- **错误不缓存**：内层返回 `Err` 时原样传播，不写入任何部分结果。
- **锁纪律**：内层 `.await` 处于两段独立 `cache.lock()` 之间，绝不跨 `.await` 持锁
  （避免 `clippy::await_holding_lock` 与死锁风险）。
- **两代有界缓存（audit F4）**：缓存由 `current` + `previous` 两个 `HashMap` 组成，各上限 `CACHE_GEN_CAP = 2048`，
  常驻项数因而被界定在约 `2*CAP`。查找先看 `current`、再看 `previous`，命中 `previous` 时**提升回 `current`**（promote-on-hit）；
  当 `current` 写满时**轮转**——`previous = current`、`current` 重置为空，旧 `previous` 整代丢弃。
  热文本（工具描述每次 rebuild 都重嵌）凭 promote-on-hit 持续驻留，而临时的 query 向量不再无界累积。
  缓存键为 **64 位内容哈希**，碰撞概率在本规模（目录小）下可忽略。
  （旧实现为**无界 `HashMap`、从不驱逐**，是本次审计修复的内存增长隐患。）

## 向量策略（M2-A T4）

`VectorStrategy`（`crates/retrieval/src/vector.rs`）在云端嵌入上做**暴力余弦检索**，内置一个
`Bm25Strategy` 作为透明降级目标：

- **归一化后余弦 = 点积**：`normalize` 对每个向量做 L2 归一化，查询向量也归一化，于是 `dot(qv, v)`
  直接就是余弦相似度，省去逐次再除范数。
  - **零范数保护**：`normalize` 仅在 `norm > 0.0` 时相除，零向量原样保留——否则除零得 `NaN`。
    嵌入器对无 token 文本可能返回零向量（T2 review 指出），零向量的余弦因此为 `0` 而非 `NaN`。
- **暴力线性扫描（目录小）**：工具目录规模小，归一化向量上的线性点积扫描完全够用，**不引入 ANN 索引**
  （YAGNI）。排序同 BM25：分数降序、同分 `qualified_name` 升序，确定性 tie-break 后 `truncate(top_k)`。
- **两条降级路径**：
  1. **degraded（索引期）**：`index` 总是**先（重）建 BM25**，再尝试批量嵌入全部工具文本；嵌入失败时
     `tracing::warn!` 记录、清空 `vectors`、置 `degraded = true`，此后所有查询走 BM25。
  2. **per-query（查询期）**：未降级时每次查询先嵌入 query，若该次嵌入失败则**仅本次**回退 BM25
     （不改变 `degraded` 状态）；若 `degraded` 或 `vectors` 为空也直接走 BM25。

## 混合策略 `HybridStrategy`（RRF）

`HybridStrategy`（`crates/retrieval/src/hybrid.rs`）用 **Reciprocal Rank Fusion** 融合 `Bm25Strategy`
（词法）与 `VectorStrategy`（语义）两份排名：

- **全深度子检索**：`search` 以 `doc_count`（索引时 = `catalog.iter().count()`）为 `top_k` 调用两个子策略，
  确保 RRF 看到每个文档在各列表中的**真实名次**；若先各自截到 `top_k` 再融合，会丢掉「一边名次低、另一边名次
  高」的单边命中。
- **RRF 公式**：`fused(doc) = Σ_L 1/(60 + rank_L(doc))`，`k=60` **固定**（不暴露配置）。按「融合分降序 +
  `qualified_name` 升序」排序后截 `top_k`。融合分量级很小，**不可跨策略比较**。
- **不对称性（有意）**：BM25 子表只含命中词项的文档（`score>0` 过滤）；向量子表含**全部**文档。故仅语义相关、
  无词法命中的工具仍能经向量表进入融合（hybrid 相对纯 BM25 的召回增益）；反之，对任意非空 query，含 embedding
  的 hybrid 总能返回最多 `top_k` 个语义最近结果（不同于纯 BM25「无命中即空」）。
- **降级自愈**：embedding 失败时（索引期或查询期），`VectorStrategy` 返回内部 BM25 排名 → 两份子表≈同一 BM25
  排名 → RRF 融合后名次单调一致 → hybrid 退化≈纯 BM25。**无需额外 degraded 标志**。

## subagent 策略 `SubagentStrategy`（retrieve-then-rerank）

`SubagentStrategy`（`crates/retrieval/src/subagent.rs`）用一个**小型 chat 模型**对 BM25 预筛出的候选做
**重排**（retrieve-then-rerank），prompt 构造与响应解析都在本 crate（纯逻辑、可经 `MockChatModel` 测试），
真实 HTTP chat 客户端在独立 `chat` crate。

- **数据流**：`index` 委托内置 `Bm25Strategy`；`search` 先 `bm25.search(query, candidates)` 取候选 shortlist →
  `build_user_prompt`（query + 编号候选清单 + "最多 top_k 个"）→ `chat.complete(SYSTEM_PROMPT, user)` →
  `parse_selection`（白名单去重保序）→ 命中名映回 `ScoredTool` 赋**合成递减分** `(n − i)` → 取前 `top_k`。
- **合成分仅承载次序**：量级与 BM25/cosine 无关，**不可跨策略比较**；它只是把模型给的有序选择编码成可排序的
  `ScoredTool.score`。
- **空 shortlist 限制（重要）**：BM25 预筛是**固定**前置步骤。一个纯语义、无任何词法命中的 query 会得到**空
  shortlist**，此时 `search` **直接返回空且不调用 chat**——subagent 复用 BM25 的召回，不额外引入语义召回（当前
  刻意取舍；若要覆盖此场景，可在 prefilter 处换成 vector/hybrid）。
- **幻觉过滤**：`parse_selection` 以 shortlist 作白名单，模型臆造（不在候选里）的工具名被丢弃；从首个 `[` 到末个
  `]` 截取 span，容忍模型在数组外夹带散文/代码围栏；非法 JSON 或非字符串数组都解析为空。
- **降级自愈**：chat 调用失败（`tracing::warn!`）、回复解析失败或解析后零个合法工具（`tracing::debug!`），均回退到
  **BM25 shortlist 截到 `top_k`**——无需额外 degraded 标志，BM25 预筛结果本就在手。
- **为何 prompt/parse 放在 retrieval**：保持 `retrieval` 无 HTTP 依赖；`ChatModel` trait 把网络隔离到 `chat`
  crate，于是重排逻辑可用确定性 `MockChatModel` 做端到端单测（脚本回复 → 断言重排次序/降级/幻觉过滤）。

## 相关

- 接口见 L2：[retrieval](../L2-components/retrieval.md)；逐文件 API 见 L4：[retrieval/lib.rs](../L4-api/retrieval-lib.md)
