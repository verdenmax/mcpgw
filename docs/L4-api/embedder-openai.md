# L4 — `crates/embedder/src/lib.rs` API

源文件：`crates/embedder/src/lib.rs`。

`OpenAiEmbedder`：由 **OpenAI 兼容** `/embeddings` 端点支撑的 `Embedder` 实现（OpenAI，或
说同样形状的本地服务器，如 Ollama / LM Studio / vLLM）。**全工作区唯一依赖 reqwest 的 crate**；
其它一切只通过 `retrieval::Embedder` trait 交互。

## `struct OpenAiEmbedder`
```rust
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    dim: Option<usize>,
}
```
调用 `POST {base_url}/embeddings`，携带 Bearer token。`dim` 设置后会被强制校验。

### `OpenAiEmbedder::new`
```rust
pub fn new(
    base_url: String,
    model: String,
    api_key: String,
    dim: Option<usize>,
    timeout: Option<Duration>,
) -> Self
```
- `base_url`：端点基址；**尾部 `/` 会被去除**（`trim_end_matches('/')`），随后拼接 `/embeddings`。
- `model`：模型名，原样放进请求体 `model` 字段。
- `api_key`：作为 `Authorization: Bearer {api_key}` 发送。
- `dim`：可选的期望维度；`Some(d)` 时对每条返回向量做长度校验。
- `timeout`：可选请求超时；`Some(t)` 时通过 `reqwest::Client::builder().timeout(t)` 设置。

## 请求形状
向 `POST {base_url}/embeddings` 发送 JSON：
```json
{ "model": "<model>", "input": ["text0", "text1", ...] }
```
`input` 为输入文本数组（顺序对应）。

## 响应形状
期望 OpenAI 兼容响应，仅解析 `data[]`：
```json
{ "data": [ { "index": 0, "embedding": [..f32..] }, ... ] }
```
内部反序列化类型（私有）：
```rust
struct EmbeddingData { index: usize, embedding: Vec<f32> }
struct EmbeddingsResponse { data: Vec<EmbeddingData> }
```
其它字段（`object`、`model`、`usage` 等）被忽略。

## `index` 排序
返回的 `data` 按 `index` **升序排序**（`sort_by_key`），使输出顺序与输入顺序一致，**与服务器返回
顺序无关**——故意乱序返回的服务器也能被正确还原。

## `dim` 校验
当 `dim = Some(expected)` 时，逐条检查 `embedding.len() == expected`；任一不符即返回
`EmbedError::Dimension { expected, got }`。`dim = None` 时跳过校验。
`fn dim(&self) -> usize` 返回 `self.dim.unwrap_or(0)`。

## 错误 → `EmbedError`
| 情形 | 映射 |
|------|------|
| 网络/发送失败 (`send`) | `EmbedError::Provider("request failed: {e}")` |
| 非 2xx 状态 | `EmbedError::Provider("HTTP {code} from embeddings endpoint")` |
| 响应解码失败 (`json`) | `EmbedError::Provider("decode failed: {e}")` |
| 返回条数 ≠ 输入条数 | `EmbedError::Provider("expected {n} embeddings, got {m}")` |
| 维度不符 | `EmbedError::Dimension { expected, got }` |

每次调用 **all-or-nothing**：要么整批成功并保序返回，要么返回 `Err`。

## 测试
`crates/embedder/tests/openai.rs` 用本地 **axum stub** 做 mock-HTTP 单测（无需真实 API key）：
stub 故意乱序返回以验证 `index` 排序，并断言请求体携带 `model` 与 `input[]`；另一个用例用期望维度
99 触发 `Dimension` 错误。
