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
