# L4 — `crates/retrieval/src/subagent.rs` API

源文件：`crates/retrieval/src/subagent.rs`。

## `struct SubagentStrategy`
```rust
pub struct SubagentStrategy { /* bm25, chat, candidates（均私有） */ }

impl SubagentStrategy {
    pub fn new(chat: std::sync::Arc<dyn ChatModel>, candidates: usize) -> Self;
}
```
**retrieve-then-rerank** 检索器：内部 BM25 先预筛出候选 shortlist，再交由一个小型 chat 模型重排。
实现 `RetrievalStrategy`（async）。一般经 `build_strategy("subagent", &Backends { chat: Some(chat), .. })` 构造。

- `new(chat, candidates)`：持有注入的 `Arc<dyn ChatModel>` 与一个新建（空）的 `Bm25Strategy`；`candidates`
  经 `.max(1)` 夹紧（至少 1），作为 BM25 预筛 shortlist 的大小。
- `pub const DEFAULT_CANDIDATES: usize = 20`：预筛 shortlist 的默认大小，factory 在 `subagent_candidates == None`
  时取此值。

- `index(&mut self, &Catalog)`：直接委托内置 `bm25.index`，构建 BM25 索引。
- `search(&self, query, top_k)`：
  1. **BM25 预筛**：`bm25.search(query, self.candidates)` 取候选 shortlist。
  2. **空 shortlist 短路**：shortlist 为空（无词法命中）→ 直接返回空，**不调用 chat**。
  3. **构造 prompt 并调 chat**：`chat.complete(SYSTEM_PROMPT, build_user_prompt(query, &shortlist, top_k))`。
  4. **解析**：成功则 `parse_selection(reply, &shortlist)`。
  5. **降级**：chat 报错、解析失败或解析后无有效工具 → 返回 BM25 shortlist 截到 `top_k`（透明降级）。
  6. **映射**：否则把选中的 qualified_names 映回 `ScoredTool`，赋**合成的递减分数**（`(n − i)`，仅表达次序），
     取前 `top_k`。

## 私有 `SYSTEM_PROMPT` / `fn build_user_prompt` / `fn parse_selection`
- `SYSTEM_PROMPT`（私有常量）：指示模型"只回一个 JSON 数组、只从候选中选、最相关在前、不超过请求数量、无散文/
  代码围栏"。
- `build_user_prompt(query, shortlist, top_k)`（私有）：拼出 `Query: {query}`、`Return at most {top_k}
  qualified_names.`，再附**编号**候选清单（`{i}. {qualified_name}: {description}`）。
- `parse_selection(reply, allowed)`（私有）：从**首个 `[` 到末个 `]`** 截取 span（容忍模型在数组外加散文/围栏），
  用 `serde_json` 解析为 `Vec<String>`；只保留出现在 shortlist（`allowed`）中的名字（**剔除幻觉**）、去重、保序；
  任一步失败（无方括号 / 非法 JSON / 非字符串数组）即返回空 `Vec`，调用方据此降级。

三者均**私有**，不属公开 API。

## 行为要点 / 限制
- **合成分数**：选中工具的 `score = (n − i)`（`n` = 选中数、`i` = 0 起的次序），仅承载**次序**，量级与 BM25/cosine
  无关，**不可跨策略比较**。
- **空 shortlist 限制**：BM25 预筛是**固定**前置。纯语义、无任何词法命中的 query 会得到空 shortlist → 直接返回空
  （并不调用 chat）。这是当前刻意取舍：subagent 复用 BM25 的召回，不额外引入语义召回。
- **幻觉过滤**：`parse_selection` 以 shortlist 作白名单，模型臆造的工具名被丢弃。
- **降级自愈**：chat 失败 / 解析失败 / 选不出合法工具，均回退到 BM25 shortlist（`tracing::warn!` / `debug!`
  记录），无需额外状态标志。

## 测试
`#[cfg(test)]` 单测覆盖 `parse_selection`：保序去重 + 剔除幻觉、garbage/空数组返回空、非字符串 JSON 数组
（`[1,2,3]`）触发 serde 解析错误返回空。端到端 search 经 `retrieval::MockChatModel`（`testkit`）注入脚本回复
（见 `crates/retrieval/tests/subagent.rs`）：重排跟随模型次序、幻觉名被丢弃、chat 失败/garbage 回复降级到 BM25、
空 shortlist 不调用 chat、重排后仍尊重 `top_k`。

> 算法/数据流见 L3：[retrieval](../L3-details/retrieval.md)。
