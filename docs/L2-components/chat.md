# L2 — `chat` 组件

## 职责

承载**真实 HTTP 后端的 chat 补全实现** `OpenAiChat`，调用 **OpenAI 兼容**的
`POST {base_url}/chat/completions` 端点（OpenAI，或同样形状的本地服务器，如 Ollama / LM Studio /
vLLM）。供 `subagent` 检索策略做"用小模型重排候选工具"。

本 crate 与 `embedder` **并列、是全工作区仅有的两个依赖 `reqwest` 的 crate**：把 HTTP/序列化的关注点隔离
在此，使 `retrieval` 等其它 crate 保持无 HTTP 依赖，只通过 `retrieval::ChatModel` trait 交互。

## 公开接口

### 类型 `OpenAiChat`
实现 `retrieval::ChatModel`。

| 项 | 签名 | 说明 |
|------|------|------|
| `new` | `(base_url, model, api_key, timeout: Option<Duration>) -> Self` | 构造客户端；`base_url` 尾部 `/` 被去除，可选超时 |
| `complete` | `async (&self, system: &str, user: &str) -> Result<String, ChatError>` | 调 `/chat/completions`（`temperature: 0`、system+user 两条消息、Bearer），返回 `choices[0].message.content` |

详见 L4：`docs/L4-api/chat-openai.md`（请求/响应形状、错误映射、≤500 字符 body 片段、空白内容收敛 `Empty`）。

## 依赖

- `retrieval`（trait `ChatModel` + `ChatError`）。
- `reqwest`（`0.13`，`default-features = false`，features `json` + `rustls`）——与 `embedder` 并列的 HTTP 依赖。
- `async-trait`、`serde`、`serde_json`。
- dev：`tokio`（`full`）、`axum`——用于 mock-HTTP 单测。

## 被谁使用

- `mcpgw`：`strategy = "subagent"` 时由 `build_backends` 从 `[retrieval.subagent]` 构造 `OpenAiChat`
  （启动期 fail-fast 读 `api_key_env`），作为 `Arc<dyn ChatModel>` 注入 `Backends.chat`，再经
  `build_strategy("subagent", &backends)` 装进 `SubagentStrategy`。

## 不负责

- 检索/排序策略与 prompt 构造/响应解析（属 `retrieval::SubagentStrategy`，纯逻辑、可经 `MockChatModel` 测试）。
- 配置解析、CLI（属 `config` / `mcpgw`）。

> 真实 HTTP 嵌入后端见姊妹组件：[embedder](./embedder.md)。
