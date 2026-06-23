# L3 — `config` 细节

## serde 属性组合的行为

`Config` 与 `RetrievalConfig` 均带 `#[serde(deny_unknown_fields)]`，`RetrievalConfig` 另有容器级
`#[serde(default)]`，`Config.retrieval` 字段也有 `#[serde(default)]`。其实际行为（已通过探针验证）：

- **部分填充**：`[retrieval]\ntop_k = 3` → `strategy` 取默认 `"bm25"`、`top_k = 3`。容器级 `default`
  会先用 `Default::default()` 铺底，再覆盖出现的字段。
- **未知字段 → `Parse`**：`[retrieval]` 内的拼写错误/多余键被 `deny_unknown_fields` 拒为
  `ConfigError::Parse`（而非静默接受）。
- **`[server]` 段**（M1-B）：`ServerConfig { stdio: bool }`，容器级 `#[serde(default, deny_unknown_fields)]`，
  `stdio` 默认 `true`。省略 `[server]` → `stdio = true`；段内未知键（无 flatten）→ `Parse`。
- 注：serde 已知的"`flatten` + `deny_unknown_fields`不兼容"在此**不适用**——`Config` / `RetrievalConfig` /
  `ServerConfig` 都不 flatten，故 `default` + `deny_unknown_fields` 兼容。

## `[[upstream]]` 的 serde 建模

`Config.upstreams`（`#[serde(rename = "upstream")]`）解析 `[[upstream]]` 数组。每个 `UpstreamConfig`
用 `#[serde(flatten)]` 把内部标签枚举 `UpstreamTransport`（`#[serde(tag = "transport")]`）摊平到同一表中，
于是 `transport = "stdio"` 这个判别字段与变体字段（`command`/`args`/`env_passthrough`）平铺共存。

- **关键约束（已踩坑记录）**：`UpstreamConfig` **不能**带 `#[serde(deny_unknown_fields)]`——`flatten` +
  `deny_unknown_fields` 是 serde 的不兼容组合。代价：`[[upstream]]` 表内的未知键被静默忽略（如 `comand`
  拼写错会被丢弃，到连接时才暴露）。`deny_unknown_fields` 只加在不 flatten 的顶层 `Config` 上。
- **未知 transport 值**（如 `transport = "carrier-pigeon"`）→ `Parse`（内部标签枚举拒绝未知判别值）。
- `args` / `env_passthrough` 省略时默认空；`call_timeout_ms` 省略时默认 `30_000`（经 flatten 路径仍正确）。

### HTTP 上游（M1-C T1）

`UpstreamTransport::Http { url, bearer_env, headers }` 是 M1-C 新增的远端 Streamable HTTP 上游变体（T1
仅扩 schema，连接逻辑在 T2+）。认证值**只经 env 变量名引用**，配置里不出现明文密钥。

- **headers 用内联表而非 `[[upstream.header]]`**：`headers` 是「头名 → 持有该头值的 env 变量名」的 TOML
  **内联表**（`headers = { "X-Api-Version" = "REMOTE_VER" }`），刻意**不**采用 `[[upstream.header]]` 这类
  数组表。原因：`UpstreamConfig` 用 `#[serde(flatten)]` 摊平内部标签枚举 `UpstreamTransport`，而
  `flatten` + 内部标签枚举对**数组表**（array-of-tables）的解析有限制，无法可靠地把 `[[upstream.header]]`
  归位到变体字段；内联表（map 字段）则能正常经 flatten 路径解析。这是 spec §10 记录的回退选项。
- `bearer_env`：可选，持有 bearer token 的 env 变量名（发为 `Authorization: Bearer <token>`），省略时 `None`。
- `url` 经 `validate()` 强制 `trim()` 后非空（否则 `Invalid`）。
- T1 阶段 `connect_all` 仍只调用 `connect_stdio_upstream`，故 `transport = "http"` 的上游会被降级 skip
  （返回 "non-stdio" 连接错误）；HTTP 连接在 T2 接上。

### `[server.http]` 段（M1-C T1）

`ServerConfig.http: Option<HttpConfig>` 省略整个 `[server.http]` → `None`（HTTP 关闭）。`HttpConfig` 与
`ApiKeyConfig` 均无 flatten，故 `#[serde(default, deny_unknown_fields)]` 兼容且生效（段内未知键 → `Parse`）。
`api_keys` 经 `#[serde(rename = "api_key")]` 映射 `[[server.http.api_key]]` 数组；只给 `enabled` 时
`bind`/`path` 取默认（`127.0.0.1:8970` / `/mcp`）、`api_keys` 为空。API key 密钥**只经 env 变量名引用**
（`ApiKeyConfig.env`），`name` 仅作日志标签、绝非密钥值。

### `[audit]` 段（M6.T3）

`Config.audit: AuditConfig`（`#[serde(default, deny_unknown_fields)]`）开关网关的**仅追加 JSONL 审计落盘**：
`enabled` 时每次元工具调用写一行**仅元数据** `CallRecord`（落盘细节见 L4
[observe-audit](../L4-api/observe-audit.md)）。

- **字段与默认**：`enabled: bool`（默认 `false`，须显式 opt-in）、`path: String`（默认 `"mcpgw-audit.jsonl"`，
  CWD 相对）。**省略整个 `[audit]` 段 → `AuditConfig::default()`（关闭）**；只给 `enabled = true` 时 `path` 取
  默认（`audit_partial_fills_defaults` 单测锁定）。无 flatten，故 `deny_unknown_fields` 生效（段内未知键如
  `bogus` → `Parse`）。`validate()` **不**校验 `path`——文件能否打开在 `serve` 启动期由
  `observe::spawn_writer` fail-fast 暴露（开不了即拒绝启动），而非配置解析期。
- **无内建轮转 / 无 SIGHUP 重开**：`JsonlSink` 只对**单一打开的文件句柄**做 append，进程运行期**不**重开文件、
  **不**响应 `SIGHUP`，也无大小/时间轮转。文件轮转须交给**外部 logrotate**，两种姿势各有取舍：
  - **① `copytruncate`**：无需停机（不重启进程），但**复制与截断之间写入的行可能丢失**（logrotate 复制完
    旧文件、再 `truncate` 清空，这两步之间 writer 以 `O_APPEND` 追加的行既不在副本里、又被 truncate 抹掉）
    ——可接受少量丢失时用。注：因以 `O_APPEND` 打开，每次写入前内核重定位到文件末尾，故**不会**出现非 append
    句柄那种 truncate 后 offset 错位、写出稀疏/空洞文件的问题；唯一损失就是上述 copy↔truncate 竞态窗口。
  - **② 停—转—起（stop → rotate → restart）**：关停 mcpgw（关停时审计 writer 会优雅 drain+flush+fsync，见
    L4 [mcpgw-main](../L4-api/mcpgw-main.md)）→ 移动/压缩旧文件 → 重启（`create+append` 建新文件）。**零丢失**，
    代价是一次重启停机。
- **单写者 / 每进程独立 path**：审计 writer 是**进程内唯一的单写者线程**；**多个进程写同一文件是误配**
  （交错/损坏行、轮转语义破裂），每个网关进程应配独立 `path`。

### `[dashboard]` 段（子系统 A）

`Config.dashboard: DashboardConfig`（`#[serde(default, deny_unknown_fields)]`）开关**默认只读可视化面板**——一个
**独立端口、localhost**的 web server，读端点无鉴权，展示快照/指标/搜索追踪（实现见 [dashboard L3](./dashboard.md)）；
另含**可选**的运行时禁用写子系统（子系统 B）的两个开关（`admin_token_env`/`disabled_state_path`）。

- **字段与默认**：`enabled: bool`（默认 `false`，须显式 opt-in）、`bind: String`（默认 `"127.0.0.1:8971"`，
  仅 localhost、读端点无 auth）、`trace_queries: bool`（默认 `false`，opt-in 后才捕获 **query 文本 + 命中工具名/分数**
  的发现追踪）、`trace_path: Option<String>`（默认 `None`，给出则把发现追踪另写一份 JSONL 供历史回放，否则仅内存
  ring buffer）、`trace_buffer: usize`（默认 `500`，内存发现 ring buffer 容量，须 `> 0`）、`call_buffer: usize`
  （默认 `2000`，逐条调用环容量，须 `> 0`）、`payload_max_bytes: usize`（默认 `16384`，单条调用 args/result 内容文本
  各自的字节封顶，须 `> 0`）、`admin_token_env: Option<String>`（默认 `None`，**持有 admin Bearer token 的环境变量名**——
  仅引用 env 名、**绝不**是 token 值；`None` → admin 写 API 关闭）、`disabled_state_path: Option<String>`（默认 `None`，
  运行时禁用集的 JSON 持久化路径；`None` → 仅内存、重启即清、**无自动默认**）。
- **省略整个 `[dashboard]` 段 → `DashboardConfig::default()`（关闭）**；只给 `enabled = true` 时其余字段取默认
  （`dashboard_defaults_and_partial_fill` 单测锁定）。无 flatten，故 `deny_unknown_fields` 生效（段内未知键如
  `bogus` → `Parse`）。`admin_token_env`/`disabled_state_path` 默认 `None` 与解析由
  `dashboard_admin_and_disabled_path_default_none` / `dashboard_parses_admin_and_disabled_path` 单测锁定。
- **隐私分层**：`trace_queries` 控的是**与审计/观测物理隔离的独立通道**——审计 JSONL（`[audit]`）与
  `observe::CallRecord` 始终**仅元数据**，绝不含 query 文本；只有 dashboard 的发现追踪（`DiscoveryRecord`）才带
  query 与工具名，且默认关闭。逐条调用内容（args/result）同理：只活在内存调用环、供详情页实时展示/过滤，绝不落盘
  （见 [downstream L3 调用内容捕获](./downstream.md) 与 [dashboard L3](./dashboard.md)）。`admin_token_env` 是**对
  env 名的引用**，配置层只透传，**绝不**含 token 值；该 token 绝不进 `/api/about`（`admin_enabled` 仅 bool）、不被日志。
- **`validate()`**：仅当 `enabled` 时校验 `bind.trim()` 非空、`trace_buffer > 0`、`call_buffer > 0`、
  `payload_max_bytes > 0`（否则 `Invalid`）；端口能否绑定在 `serve` 启动期由**预绑定监听**fail-fast 暴露，而非配置
  解析期。`admin_token_env` **不**经 `validate()`——其 env 变量在 `serve` 启动期 **fail-fast 解析**（`resolve_admin_token`，
  仅当 `dashboard.enabled`；env 缺失/空/全空白 → 报错，详见 [mcpgw-main L4](../L4-api/mcpgw-main.md)）；
  `disabled_state_path` 由 gateway 在装配期 `DisableSet::load_or_new` 读取（坏文件自愈，**独立于 `enabled`**）。
- **M5 在线改配的校验复用**（边界说明）：dashboard 子系统 C 的在线编辑器对提交的整份 TOML 复用**同一套** `from_toml_str`
  结构校验 + `serve` 启动期的 env 解析器（经 `main.rs::validate_config_text` 注入）。`config` crate **不**为此新增任何
  API、仍只做纯解析/结构校验，env 值的读取与 fail-fast 始终在 `mcpgw` bin——这条边界（config 纯解析、env 解析在 serve）
  在 M5 保持不变。

## `env_passthrough` 的 allow-list 语义

`env_passthrough` 不是「额外追加」而是「白名单」：`upstream::connect::build_command` 先 `c.env_clear()` 清空子进程
环境，再仅把 `env_passthrough` 列出、且在 mcpgw 自身环境里存在的变量逐个注入。因此子进程默认**继承不到**父进程
任何环境变量——这是有意的最小权限默认（避免凭据/路径意外泄漏给上游子进程）；需要 `PATH`/`HOME`/凭据变量的上游须
在配置里显式列出。该行为由 `upstream` 的 `build_command_applies_env_allowlist` 单测锁定。

## 校验逻辑 `validate`（私有）

- `strategy` 必须 ∈ `["bm25", "vector", "hybrid", "subagent"]`，否则 `Invalid`。
- `top_k` 必须 `> 0`，否则 `Invalid`。
- 每个 upstream 的 `name`：`trim()` 后非空（拒绝纯空白）、不含命名空间分隔符 `__`、**不以 `_` 开头或结尾**、在所有 upstream 中
  唯一（重复 → `Invalid`）。
  - **边界下划线**单独拒绝：`my_server` 这类内部下划线允许，但 `_svc`/`svc_` 会在拼接 `{server}__{tool}` 时与相邻的 `_`
    重新拼出 `__` 分隔符，破坏命名空间唯一性，故 `starts_with('_') || ends_with('_')` → `Invalid`。
- 每个 upstream 的 `call_timeout_ms` 必须 `> 0`，否则 `Invalid`：`0` 会让连接握手与每次调用的
  `tokio::time::timeout(Duration::from_millis(0), …)` **立即** `Elapsed`，使该上游不可用——与其他数值旋钮一致地拒绝。
- `[server.http].path`（若有 `[server.http]` 段）：必须以 `/` 开头且长于 `/`（即 `len >= 2`），且**不得含通配/参数段**
  （不含 `{`、`}`、`*`），否则 `Invalid`。这在**启动期、axum `nest_service` 之前**校验，拒绝 `""`/`"/"`/无前导斜杠的挂载路径，
  以及 `/{id}`、`/{*rest}`、`/a*b` 这类含动态段的路径——否则 axum 会在构建路由时 panic（`/{*rest}`）或把 MCP 静默挂到
  动态捕获段（`/{id}`）上；默认 `/mcp` 与 `/a/b/c`、`/mcp-v1` 等普通字面量路径不受影响。

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
  `rejects_blank_upstream_name` / `rejects_duplicate_upstream_names` /
  `rejects_server_name_leading_or_trailing_underscore` / `accepts_server_name_with_interior_underscore`（均 `Invalid` / 接受内部下划线）
- `rejects_invalid_http_path`（`""`/`"/"`/无前导斜杠均 `Invalid`）/ `rejects_wildcard_or_param_http_path`（`/{id}`、`/{*rest}`、`/a*b` 等含 `{`/`}`/`*` 均 `Invalid`）/ `accepts_plain_literal_http_paths`（`/mcp`、`/a/b/c`、`/mcp-v1` 等普通路径通过）/ `accepts_default_and_custom_http_path`（`/mcp`、`/gateway` 通过）
- `server_section_parses_and_defaults_to_stdio`（`[server]` 缺省 `stdio = true`、显式解析、未知键 → 错误）
- `audit_defaults_disabled`（省略 `[audit]` → `enabled = false`、`path = "mcpgw-audit.jsonl"`）/
  `parses_audit_section`（显式 `enabled`/`path` 解析）/ `audit_rejects_unknown_field`（段内未知键 → `Parse`）/
  `audit_partial_fills_defaults`（只给 `enabled` 时 `path` 取默认）
- `dashboard_defaults_and_partial_fill`（只给 `enabled` 时 `bind`/`trace_queries`/`trace_path`/`trace_buffer` 取默认
  `127.0.0.1:8971`/`false`/`None`/`500`）/ `omitting_dashboard_section_is_disabled`（省略 `[dashboard]` → 关闭）/
  `dashboard_rejects_unknown_field_and_zero_buffer`（段内未知键 → `Parse`；`trace_buffer = 0` → `Invalid`）

## 相关

- 接口见 L2：[config](../L2-components/config.md)；逐文件 API 见 L4：[config/lib.rs](../L4-api/config-lib.md)
