# L2 — `embedder` 组件

## 职责

承载**真实 HTTP 后端的嵌入实现** `OpenAiEmbedder`，调用 **OpenAI 兼容**的
`POST {base_url}/embeddings` 端点（OpenAI，或同样形状的本地服务器，如 Ollama / LM Studio /
vLLM）。

本 crate 与 `chat` **并列、是全工作区仅有的两个依赖 `reqwest` 的 crate**：把 HTTP/序列化的关注点隔离在此，使
`retrieval` 等其它 crate 保持无 HTTP 依赖，只通过 `retrieval::Embedder` trait 交互。

## 公开接口

### 类型 `OpenAiEmbedder`
实现 `retrieval::Embedder`。

| 项 | 签名 | 说明 |
|------|------|------|
| `new` | `(base_url, model, api_key, dim: Option<usize>, timeout: Option<Duration>) -> Self` | 构造客户端；`base_url` 尾部 `/` 被去除，可选超时 |
| `embed` | `async (&self, &[String]) -> Result<Vec<Vec<f32>>, EmbedError>` | 调 `/embeddings`，按 `index` 排序，保序返回 |
| `dim` | `(&self) -> usize` | `self.dim.unwrap_or(0)` |

详见 L4：`docs/L4-api/embedder-openai.md`（请求/响应形状、`index` 排序、`dim` 校验、错误映射）。

## 依赖

- `retrieval`（trait `Embedder` + `EmbedError`）。
- `reqwest`（`0.13`，`default-features = false`，features `json` + `rustls`）——与 `chat` 并列的 HTTP 依赖。
- `async-trait`、`serde`、`serde_json`。
- dev：`tokio`（`full`）、`axum`——用于 mock-HTTP 单测。

## 不负责

- 检索/排序策略（属 `retrieval`）。
- 配置解析、CLI（属 `config` / `mcpgw`）。
- 嵌入缓存（属 `retrieval::CachingEmbedder`，可装饰本 crate 的实例）。

> 真实 HTTP chat（重排）后端见姊妹组件：[chat](./chat.md)。
