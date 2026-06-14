# L4 — `crates/chat/src/lib.rs` API

源文件：`crates/chat/src/lib.rs`。

`OpenAiChat`：由 **OpenAI 兼容** `/chat/completions` 端点支撑的 `ChatModel` 实现（OpenAI，或说同样形状的本地
服务器，如 Ollama / LM Studio / vLLM）。**与 `embedder` 并列、是全工作区仅有的两个依赖 reqwest 的 crate**；
其它一切只通过 `retrieval::ChatModel` trait 交互。

## `struct OpenAiChat`
```rust
pub struct OpenAiChat {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}
```
调用 `POST {base_url}/chat/completions`，携带 Bearer token，以 `temperature: 0`（输出稳定）请求，返回
`choices[0].message.content`。

### `OpenAiChat::new`
```rust
pub fn new(
    base_url: String,
    model: String,
    api_key: String,
    timeout: Option<Duration>,
) -> Self
```
- `base_url`：端点基址；**尾部 `/` 会被去除**（`trim_end_matches('/')`），随后拼接 `/chat/completions`。
- `model`：模型名，原样放进请求体 `model` 字段。
- `api_key`：作为 `Authorization: Bearer {api_key}` 发送。
- `timeout`：可选请求超时；`Some(t)` 时通过 `reqwest::Client::builder().timeout(t)` 设置。

## 请求形状
向 `POST {base_url}/chat/completions` 发送 JSON：
```json
{
  "model": "<model>",
  "temperature": 0,
  "messages": [
    { "role": "system", "content": "<system>" },
    { "role": "user", "content": "<user>" }
  ]
}
```
两条消息固定为 system + user 顺序；`temperature: 0` 让重排尽量确定。

## 响应形状
期望 OpenAI 兼容响应，仅解析 `choices[0].message.content`：
```json
{ "choices": [ { "message": { "content": "..." } } ] }
```
内部反序列化类型（私有）：
```rust
struct ChoiceMessage { content: Option<String> }
struct Choice { message: ChoiceMessage }
struct ChatResponse { choices: Vec<Choice> }
```
其它字段（`role`、`index`、`usage` 等）被忽略。

## 错误 → `ChatError`
| 情形 | 映射 |
|------|------|
| 网络/发送失败 (`send`) | `ChatError::Provider("request failed: {e}")` |
| 非 2xx 状态 | `ChatError::Provider("HTTP {code} from chat endpoint: {snippet}")` |
| 响应解码失败 (`json`) | `ChatError::Provider("decode failed: {e}")` |
| `choices` 为空，或 `content` 缺失/**仅空白** | `ChatError::Empty` |
| 取到非空白 `content` | `Ok(content)` |

非 2xx 时会读取响应体并截断（**≤500 字符**）拼入错误信息，保留 OpenAI 兼容服务器返回的可操作错误详情
（如 `{"error":{"message":"bad model xyz"}}`）。响应体不含请求的 `Authorization` 头，**不会泄露 api_key**。
`content` 经 `s.trim().is_empty()` 判定：纯空白内容**不**算成功，收敛为 `ChatError::Empty`（而非返回空串）。

## 测试
`crates/chat/tests/openai_chat.rs` 用本地 **axum stub** 做 mock-HTTP 单测（无需真实 API key）：断言请求体携带
`model`、`temperature: 0`、`messages[system,user]`，并捕获请求头断言服务器收到 `authorization: Bearer sk-x`；
另有用例覆盖非 2xx 携带状态码 + body 片段、`choices: []` 与**仅空白** content 均收敛为 `ChatError::Empty`。
