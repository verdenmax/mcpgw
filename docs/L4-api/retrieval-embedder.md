# L4 — `crates/retrieval/src/embedder.rs` API

源文件：`crates/retrieval/src/embedder.rs`。

嵌入抽象：把文本转成向量。**HTTP 实现位于独立的 `embedder` crate**；本模块只定义
trait、错误类型，以及一个确定性的 `MockEmbedder`（在 `testkit` feature 后面）供测试用。

## `enum EmbedError`
```rust
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding provider error: {0}")]
    Provider(String),
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    Dimension { expected: usize, got: usize },
}
```
保持 **provider 无关**，使 `retrieval` 无需任何 HTTP 依赖。
- `Provider(String)`：底层提供方（网络/认证/解析等）失败的统一封装。
- `Dimension { expected, got }`：返回维度与期望维度不一致。

## `trait Embedder`
```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
    fn dim(&self) -> usize;
}
```
把一批文本各转成一个向量（**顺序对应**输入）。每次调用 **all-or-nothing**：要么整批成功，
要么返回 `Err`。`dim()` 返回期望的嵌入维度（用于一致性检查）。

## `struct MockEmbedder`（`#[cfg(feature = "testkit")]`）
```rust
pub struct MockEmbedder {
    dim: usize,
    fail: bool,
    pub calls: Arc<AtomicUsize>,
    pub seen: Arc<Mutex<Vec<String>>>,
}
```
确定性测试嵌入器，**仅在 `testkit` feature 下导出**。
- `pub fn new(dim: usize) -> Self`：正常 mock。
- `pub fn failing(dim: usize) -> Self`：`embed` 恒返回 `Err(EmbedError::Provider(...))`，用于降级测试。

**伪向量算法**：把文本经 `crate::tokenize`（小写 + 按非字母数字切分）拆为 token，每个 token 用
**FNV-1a** 哈希到 `dim` 个桶之一，对应分量 `+= 1.0`。因此**共享 token 越多的文本余弦相似度越高**，
便于后续任务确定性地断言排序。

**可观测性**（供缓存测试用）：
- `calls: Arc<AtomicUsize>`：`embed` 被调用的次数（每次 `+1`，包括失败调用）。
- `seen: Arc<Mutex<Vec<String>>>`：成功调用所嵌入过的全部文本（失败调用不记录）。
  缓存任务用它断言内层 embedder 只对**新文本**被调用。

> `vec_for` 为私有辅助方法，不属于公开 API。

## `struct CachingEmbedder`

源文件：`crates/retrieval/src/caching.rs`。

```rust
pub struct CachingEmbedder {
    inner: Arc<dyn Embedder>,
    cache: Mutex<HashMap<u64, Arc<[f32]>>>,
}

impl CachingEmbedder {
    pub fn new(inner: Arc<dyn Embedder>) -> Self;
}

#[async_trait]
impl Embedder for CachingEmbedder { /* embed + dim */ }
```

包装任意 `Arc<dyn Embedder>` 的 **`Embedder` 装饰器**，按文本内容哈希记忆向量，使重复/未变的工具文本
只被嵌入一次（跨快照重建复用）。

- `pub fn new(inner: Arc<dyn Embedder>) -> Self`：以空缓存包装内层 embedder。
- `dim()`：直接透传 `inner.dim()`。

**缓存语义**（`embed`）：
- **缓存键 = 文本内容哈希**（FNV-1a，`hash_text`）；相同内容 → 同一缓存项。
- **仅嵌未命中**：先按内容哈希在缓存中查找，只把**唯一的未命中文本**（按首次出现顺序去重）转发给 `inner`，
  命中文本不再调用内层。
- **保序还原**：最终结果按**原始输入顺序**重组（含重复项），同一文本得到同一向量。
- **全命中跳过内层**：若整批都命中缓存，则**完全不调用** `inner.embed`（节省一次网络往返）。
- **错误不缓存**：`inner.embed` 返回 `Err` 时直接向上传播，缓存保持不变（不写入任何部分结果）。

**锁纪律**：内层 `inner.embed().await` 发生在**两段独立的 `cache.lock()` 作用域之间**——绝不跨 `.await`
持有 std `Mutex` guard（既是 `clippy::await_holding_lock` 错误，也是正确性隐患）。

> 后续任务令 `GatewayState` 持有 `Arc<CachingEmbedder>`，使缓存跨快照重建持续存在：每次 `list_changed`
> 只嵌入新增的工具文本。
