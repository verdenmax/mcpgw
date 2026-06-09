# L3 — `config` 细节

## serde 属性组合的行为

`Config` 与 `RetrievalConfig` 均带 `#[serde(deny_unknown_fields)]`，`RetrievalConfig` 另有容器级
`#[serde(default)]`，`Config.retrieval` 字段也有 `#[serde(default)]`。其实际行为（已通过探针验证）：

- **部分填充**：`[retrieval]\ntop_k = 3` → `strategy` 取默认 `"bm25"`、`top_k = 3`。容器级 `default`
  会先用 `Default::default()` 铺底，再覆盖出现的字段。
- **未知字段 → `Parse`**：`[retrieval]` 内的拼写错误/多余键被 `deny_unknown_fields` 拒为
  `ConfigError::Parse`（而非静默接受）。
- **未知顶层段**（如 `[server]`，留待 M1-B）同样被拒为 `Parse`。
- 注：serde 已知的"`flatten` + `deny_unknown_fields`不兼容"在此**不适用**——`Config` 是 `default` +
  `deny_unknown_fields`，二者兼容。

## `[[upstream]]` 的 serde 建模

`Config.upstreams`（`#[serde(rename = "upstream")]`）解析 `[[upstream]]` 数组。每个 `UpstreamConfig`
用 `#[serde(flatten)]` 把内部标签枚举 `UpstreamTransport`（`#[serde(tag = "transport")]`）摊平到同一表中，
于是 `transport = "stdio"` 这个判别字段与变体字段（`command`/`args`/`env_passthrough`）平铺共存。

- **关键约束（已踩坑记录）**：`UpstreamConfig` **不能**带 `#[serde(deny_unknown_fields)]`——`flatten` +
  `deny_unknown_fields` 是 serde 的不兼容组合。代价：`[[upstream]]` 表内的未知键被静默忽略（如 `comand`
  拼写错会被丢弃，到连接时才暴露）。`deny_unknown_fields` 只加在不 flatten 的顶层 `Config` 上。
- **未知 transport 值**（如 `transport = "carrier-pigeon"`）→ `Parse`（内部标签枚举拒绝未知判别值）。
- `args` / `env_passthrough` 省略时默认空；`call_timeout_ms` 省略时默认 `30_000`（经 flatten 路径仍正确）。

## 校验逻辑 `validate`（私有）

- `strategy` 必须 ∈ `["bm25", "vector", "hybrid"]`，否则 `Invalid`。
- `top_k` 必须 `> 0`，否则 `Invalid`。
- 每个 upstream 的 `name`：`trim()` 后非空（拒绝纯空白）、不含命名空间分隔符 `__`、在所有 upstream 中
  唯一（重复 → `Invalid`）。

> **已知的双清单**：`config::validate` 接受 `vector`/`hybrid`（"格式已知"），而 `retrieval::build_strategy`
> 仅实现 `bm25`（"是否实现"）。两份清单独立、可能漂移。这是有意的职责划分（config 管"名字合不合法"，
> 工厂管"实现没实现"），但 M1 应让"是否实现"成为单一真相源（见路线图遗留项①）。

## `default_from_empty` 的 `expect` 安全性

`default_from_empty()` 解析空串 `""`，应用全部 `#[serde(default)]`（`bm25` / `8`），二者都过 `validate`，
故 `expect("empty config is always valid")` 不可能触发——该不变量另有单测 `empty_config_uses_defaults`
独立锁定。

## 错误分层

`ConfigError::Parse`（`#[from] toml::de::Error`）与 `ConfigError::Invalid(String)` 清晰区分"语法/未知字段"
与"语义非法"，且 `Invalid` 消息带上违例值与期望集合，便于定位。

## 测试覆盖

- `empty_config_uses_defaults` / `parses_retrieval_section`
- `rejects_unknown_strategy` / `rejects_zero_top_k`（均 `Invalid`）
- `rejects_unknown_field_as_parse_error`（`Parse`）
- `partially_specified_section_fills_defaults`（部分填充）
- `parses_stdio_upstreams` / `upstreams_default_to_empty`
- `parses_explicit_call_timeout_through_flatten`（锁定 flatten 数值路径）
- `rejects_unknown_transport`（`Parse`）/ `rejects_upstream_name_with_double_underscore` /
  `rejects_blank_upstream_name` / `rejects_duplicate_upstream_names`（均 `Invalid`）

## 相关

- 接口见 L2：[config](../L2-components/config.md)；逐文件 API 见 L4：[config/lib.rs](../L4-api/config-lib.md)
