# M2-A 设计：向量检索基础（异步策略 + Embedder + 缓存 + VectorStrategy）

> 状态：已通过 brainstorm 评审，待 writing-plans 细化为实现计划。
> 前置：M0 / M1（A/B.1/B.2/C）已合并到 master（HEAD `6b5ddb8`）。
> 关联里程碑：roadmap `M2`（检索深度）的第一块 M2-A。

## 1. 目标与范围

把检索从"仅 BM25 同步"升级为"可插拔、异步、失败可降级"的架构，并落地真正可用的云向量检索。

**范围内：**
- `RetrievalStrategy` 改为 **async**（经 `async-trait` 保持 `Box<dyn>` 对象安全）。
- 新增 `Embedder` 抽象 + `CachingEmbedder`（内容哈希缓存）+ `MockEmbedder`（确定性测试）。
- 新增 `VectorStrategy`：暴力余弦检索，**内置 BM25 索引做透明降级**。
- 新增真实 **OpenAI-兼容** embedder（独立 `embedder` crate，HTTP 仅在此）。
- 新增 config `[retrieval.vector]`；`strategy = "vector"` 落地后即可用。

**明确不含（留 M2-B）：**
- `HybridStrategy`（RRF 融合 BM25+vector）。
- 默认策略切到 vector/hybrid（M2-A 默认仍为 `bm25`，vector 经 config 显式开启）。
- 嵌入缓存的磁盘持久化、查询向量 LRU、ANN 近似索引（工具目录小，暴力余弦足够）。

## 2. crate 划分（把 HTTP 关进新 crate，retrieval 保持纯净）

| crate | 新增/改动 |
|------|-----------|
| `retrieval` | `Embedder` trait + `EmbedError`；`VectorStrategy`(+内置 BM25 降级)；`CachingEmbedder`(内容哈希缓存)；`MockEmbedder`(testkit feature)；`RetrievalStrategy` 改 async（`async-trait`）；`build_strategy(name: &str, Option<&Arc<dyn Embedder>>)` |
| `embedder`（**新 crate**）| `OpenAiEmbedder`：reqwest 调 `POST {base_url}/embeddings`，实现 `retrieval::Embedder`。HTTP/序列化只在此 crate |
| `config` | `[retrieval.vector]` 结构（base_url/model/api_key_env/dim?/timeout_ms?/batch_size?）+ 校验 |
| `gateway` | `index/search` 异步化；`GatewayState` 注入 `Option<Arc<dyn Embedder>>`；`rebuild_snapshot` 用 embedder 建 vector |
| `metatools` | `search_tools` 改 async（snapshot 持有的 strategy.search 异步）|
| `downstream` | `call_tool` 的 `search_tools` 臂 `await`（该臂本就 async，无签名外溢）|
| `mcpgw` | 从 config 建 embedder（启动期 fail-fast 读 key）→ 注入 `GatewayState`；CLI `search` 用现成 runtime 包一层；验证脚本 + 门控真实冒烟 |

> retrieval 因 `async-trait` 增加该依赖；`embedder` crate 引入 reqwest+serde。retrieval **不** 依赖 reqwest。

## 3. 抽象与数据流

### 3.1 `Embedder`（retrieval，async）
```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts → one vector each (same order). All-or-nothing per call.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// Expected embedding dimension (used for sanity checks).
    fn dim(&self) -> usize;
}
```

### 3.2 `RetrievalStrategy`（改 async）
```rust
#[async_trait]
pub trait RetrievalStrategy: Send + Sync {
    async fn index(&mut self, catalog: &Catalog);
    async fn search(&self, query: &str, top_k: usize) -> Vec<ScoredTool>;
}
```
- `Bm25Strategy`：实现体不变，仅包进 `async`（无 await）。
- `VectorStrategy { embedder: Arc<dyn Embedder>, bm25: Bm25Strategy, vectors: Vec<(String /*qname*/, String /*desc*/, Arc<[f32]>)>, degraded: bool }`：
  - `index`：先建内部 `bm25`；再对每个工具文本（`qname + "\n" + description`）`embedder.embed(...)`；成功 → 存**归一化**向量、`degraded=false`；失败 → `warn!` + `degraded=true`（vectors 空）。
  - `search`：若 `degraded` 或本次 `embed([query])` 失败 → 返回 `bm25.search(query, top_k)`；否则对归一化查询向量做暴力余弦（点积），按分降序 + qname 次序稳定排序，截 top_k。

### 3.3 数据流
```
client search_tools(q) → downstream(async) → metatools::search_tools(snap,q,k).await
  → snap.strategy.search(q,k).await
     vector: embed(q) → 余弦排序 → top_k    （embed 失败/degraded → bm25.search）
重建(rebuild_snapshot, async): build_strategy(kind, embedder) → strat.index(&catalog).await
  （vector 在此批量嵌入，命中缓存只补新增）→ ArcSwap.store
```

### 3.4 构造与异步涟漪
- `GatewayState::new(strategy_name)` 保持同步且**不在构造时 index**（空目录搜索本就返回空），首个真实快照由异步 `rebuild_snapshot` 建 —— 因此 `new` 无需 await，现有 `new("bm25")` 测试不变。
- vector 路径用新构造 `GatewayState::with_embedder(strategy_name, Arc<dyn Embedder>)`（或等价 builder）；`GatewayState` 存 `strategy_name: Arc<str>` + `embedder: Option<Arc<dyn Embedder>>`，rebuild 每次用其构建策略。
- `metatools::search_tools` 改 `async`；`downstream` 的 search 臂 `await`（已 async）；`mcpgw` CLI `search` 子命令用 `tokio::runtime` 包一层（serve 已有先例）。

## 4. 缓存与降级

### 4.1 `CachingEmbedder`
```rust
pub struct CachingEmbedder { inner: Arc<dyn Embedder>, cache: Mutex<HashMap<u64, Arc<[f32]>>> }
```
- key = `hash(text)`（工具文本内容；内容变才重嵌）。
- `embed(texts)`：拆分命中/未命中 → 仅对未命中调 `inner.embed` → 回填缓存 → 按**原顺序**拼回。
- 缓存随 `GatewayState` 的 `Arc<dyn Embedder>` **跨 rebuild 存活**；list_changed 重建只嵌新增/改动工具。M2-A 仅内存。
- 查询向量通常 miss（每次 search 一次上游 API 调用），可接受；query LRU 留后续。

### 4.2 降级（呼应 §3.2）
- 索引期嵌入失败 → 该快照纯 BM25（`degraded=true`），下次 rebuild 再试。
- 查询期 `embed([query])` 失败 → 该次查询走 BM25。
- 两条都 `warn!`，**永不让 search 报错或返空**。

## 5. config Schema
```toml
[retrieval]
strategy = "vector"            # bm25(默认) | vector ; "hybrid" 仍保留 → NotImplemented(M2-B)
top_k = 8

[retrieval.vector]
base_url = "https://api.openai.com/v1"
model = "text-embedding-3-small"
api_key_env = "OPENAI_API_KEY"
# dim = 1536          # 可选；设了则做维度校验
# timeout_ms = 10000  # 可选
# batch_size = 64     # 可选，单次 embed 上限
```
- `strategy = "vector"` 时 `[retrieval.vector]` 必填，且 `base_url`/`model`/`api_key_env` 非空（config 结构校验）。
- **密钥只经 env**（M1-C 约定）；`api_key_env` 指向的值在 mcpgw 启动期解析，缺失 fail-fast（指明字段/env 名，绝不打印值）。
- 新结构（`VectorConfig`）无 `#[serde(flatten)]`，加 `#[serde(deny_unknown_fields)]`。

## 6. 错误处理
| 场景 | 处理 |
|------|------|
| `api_key_env` 缺失 | 启动 fail-fast（字段/env 名，不泄露值）|
| `strategy="vector"` 但缺 `[retrieval.vector]` 或必填项空 | config 校验失败（`ConfigError::Invalid`）|
| 索引期嵌入失败（API/网络/超时）| 降级该快照为 BM25，`warn!`，下次 rebuild 重试 |
| 查询期嵌入失败 | 该查询走 BM25，`warn!` |
| 维度不符（配了 `dim`）| 首次嵌入即 `EmbedError`，索引期 → 降级 BM25 + warn |
| `strategy="hybrid"` | `StrategyError::NotImplemented`（M2-B）|
| 上游 embeddings 非 2xx / 解析失败 | `EmbedError`，按上面降级路径处理 |

## 7. 测试策略
- **确定性单测（无网络）**：`MockEmbedder` 按文本哈希生成稳定伪向量（可构造"语义相近"对）→ 验证 VectorStrategy 余弦排序、降级（mock 返 `Err` → 落 BM25）、维度校验。
- **CachingEmbedder**：命中/未命中拆分、顺序还原、跨 rebuild 复用（用计数 mock 断言 inner 只被调用于新增文本）。
- **config**：解析 `[retrieval.vector]`；`strategy="vector"` 缺段 → 校验失败；`deny_unknown_fields` 生效。
- **gateway/metatools**：async search 全链路（mock embedder）；`new("bm25")` 等旧测试转 async 后仍绿。
- **embedder crate**：`OpenAiEmbedder` 用 mock HTTP（本地 axum/wiremock-风格 stub）断言请求体（model+input[]）与响应解析，无需真实 key。
- **门控真实冒烟** `#[ignore]`：配真实 OpenAI-兼容端点（env key）→ 嵌入工具目录 → 语义 query 排序合理；无 key/网络则跳过。
- **验证脚本**：`scripts/` 下脚本先打真实 embeddings API、在工具目录上人工核对余弦排序，再下沉 Rust（呼应路线图"脚本先验证再下沉"）。

## 8. 文档（L1–L4，随码提交，纳入双重审查）
- **L1**：检索从"仅 BM25"→"可插拔 BM25/Vector，异步、失败降级"；架构图加 `embedder` crate 与向量检索路径。
- **L2**：`retrieval`（Embedder/VectorStrategy/CachingEmbedder 职责）、新 `embedder`、`config`(新段)。
- **L3**：async 重构要点、降级两条路径、缓存键与跨 rebuild 复用、归一化余弦、`GatewayState` embedder 注入。
- **L4**：`Embedder`/`VectorStrategy`/`CachingEmbedder`/`MockEmbedder`/`OpenAiEmbedder`/`VectorConfig`/`build_strategy` 新签名。

## 9. 任务预览（writing-plans 细化为 TDD 步骤）
1. `async-trait` 重构 `RetrievalStrategy`（BM25 异步化）+ 全链路 await（metatools/gateway/downstream/mcpgw CLI）+ 旧测试转绿。
2. `Embedder` trait + `EmbedError` + `MockEmbedder`(testkit feature) + 单测。
3. `CachingEmbedder` + 缓存单测（命中/顺序/跨 rebuild 复用）。
4. `VectorStrategy`（归一化余弦 + 内置 BM25 + 双降级）+ mock 单测。
5. `build_strategy(name, embedder)` + `GatewayState::with_embedder` 注入 + `rebuild_snapshot` 用 vector。
6. config `[retrieval.vector]` + 校验 + mcpgw 启动期建 embedder（fail-fast key）+ 注入 state。
7. `embedder` crate：`OpenAiEmbedder`(reqwest) + mock-HTTP 单测。
8. 验证脚本 + 门控真实冒烟测试。
9. L1–L4 文档收口 + 全量验证（fmt/clippy/test）。

## 10. 开工前仍需在实现计划里固化的点
- `async-trait` 对 `Box<dyn RetrievalStrategy>` 的 Send 约束（search future 跨 await 持有 `Arc<GatewaySnapshot>`，需 Send）——计划首个 task 编译实证。
- `OpenAiEmbedder` 的精确请求/响应形状（`/embeddings`：`{model, input:[...]}` → `{data:[{embedding:[...]}], ...}`），用 mock HTTP 固化。
- `GatewayState` 注入 embedder 的精确构造签名（`with_embedder` vs builder），保持 `new("bm25")` 向后兼容、现有测试不破。
- `MockEmbedder` 伪向量生成方式（需让"语义相近"文本余弦更高，以便断言排序）——可用 token 集合的稀疏向量或哈希分桶。
