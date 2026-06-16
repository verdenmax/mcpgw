# mcpgw 第二轮全文审计修复（pass-2）设计

日期：2026-06-16
状态：已批准（待实现）

## 背景

第一轮审计的 16 项发现（0 Critical / 6 Important / 10 Minor）已全部修复并合并入 master。
随后对整个工作区（11 crate、~8200 行）做了一次**全文大审计**（逐 crate 通读源码、对照 rmcp 1.7
生命周期、实测 axum 路径行为、确认 189 测试通过）。结论：代码库整体健壮，**无 Critical / 无 Important**，
仅 3 项 **Minor**。本 spec 覆盖这 3 项的修复。

三项均为低影响（运维误配或半可信输入边缘），但都值得按本仓库一贯的「fail-fast + 干净错误」「对不可信
上游设防」标准收口。

## 三项发现与修复

### N1 — `[server.http].path` 校验未兑现文档承诺（config/http）

**现状**：`HttpConfig.path` 的文档注释承诺「no wildcard/`{param}` segments；validated at startup before it
reaches axum」，但 `Config::validate()`（`crates/config/src/lib.rs:319-326`）只校验了前缀 `/` 与长度
`>= 2`，**未拒绝** `{param}` / `{*wildcard}`。该 path 随后被直接交给
`axum::Router::new().nest_service(path, service)`（`crates/downstream/src/http.rs:72`）。

**触发与影响**（实测 axum 0.8.9，即 lock 版本）：
- `path = "/{*rest}"` 能过 `validate()`，但让 axum 在 `build_router` **启动期 panic**（`build_router` 在
  TCP listener 绑定之后、`run_serve` 内同步调用，无 `catch_unwind`）。这违反本仓库「每个配置错误都产出
  干净 `ConfigError::Invalid`」的一贯契约。
- `path = "/{id}"` 不 panic，但把 MCP 端点**静默挂到单段动态捕获**（匹配 `/anything`），而非运维意图的字面
  路径——一种安静的错挂。
- 仅配置可触发，**不可远程触发**。

**修复**：在 `Config::validate()` 现有 path 检查之后，新增一条：若 `http.path` 含 `{`、`}` 或 `*` 任一字符，
返回 `ConfigError::Invalid`，消息明确指出不得含通配/参数段。这样文档承诺被真正强制执行，失败退化为干净的
fail-fast 错误（与其余配置错误一致）。

**测试**：拒绝 `/{id}`、`/{*rest}`、`/a*b`、`/x{y}`；接受 `/mcp`、`/a/b/c`、`/mcp-v1`、`/v1.0/mcp`。

### N2 — 对上游 ingest 的工具数量/文本大小无上限（upstream/gateway）

**现状**：`ingest_into` → `ingest_tools`（`crates/upstream/src/mapping.rs:23-34`）原样接受上游
`list_all_tools()` 返回的一切，对**工具数量**与**单工具描述/schema 大小**均无上限；`rebuild_snapshot`
（`crates/gateway/src/lib.rs:142-167`）再把每个工具克隆进合并 catalog 并索引。per-ingest 的
`tokio::time::timeout` 只限**时间**不限**体量**——一个被攻陷/有 bug 的上游可在超时内快速返回超大工具列表。

**触发与影响**：聚合型网关的威胁面本就包含「半可信上游」。一个恶意/异常上游可驱动 catalog/快照**内存无界
增长**；在 `vector`/`hybrid` 策略下还会在每次重建时嵌入无界文本（成本/延迟；`CachingEmbedder` 仅 ~2·2048
上限，超出即抖动）。这是全代码库中「不可信输入 → 无界增长」唯一可达点。

**修复**（硬编码常量，drop+warn，复用现有 duplicate-skip 遥测风格）。在 `mapping.rs` 增加两个常量：
- `MAX_TOOLS_PER_SERVER: usize = 1024`：单次 ingest 接受的工具数达到上限后，**丢弃其余**工具并 **warn 一次**
  （含被丢弃数量与 server 名）。
- `MAX_TOOL_TEXT_BYTES: usize = 65536`（64 KiB）：某工具的 `description` 字节数 + 序列化后的 `input_schema`
  字节数若 **超过** 该上限，则**跳过该工具**（drop+warn，含 server/tool 名与实际字节数），不写入 catalog。

`ingest_tools` 的检查顺序：先 count-cap（已达上限即停止接受、warn、break），再现有的 intra-server dupe 检查，
再 byte-cap。函数返回值（`usize` 的 intra-server 重复计数；下游 `rebuild_snapshot` 已忽略该值）**语义不变**，
cap 丢弃仅经 `tracing::warn!` 上报，不改返回签名（保持改动面最小）。

byte 计算：`description` 取 `tool.description.as_deref().unwrap_or("").len()`；schema 取
`serde_json::to_string(&*tool.input_schema)` 成功时的字节长度（序列化失败按 0 计，因为这种 schema 无论如何
会在别处暴露），两者相加与 `MAX_TOOL_TEXT_BYTES` 比较。

**测试**：构造 > 1024 个工具 → 仅前 1024 个被接受；构造一个描述/schema 超 64 KiB 的工具 → 被排除而其余正常
工具仍入 catalog；边界附近（恰好等于上限、恰好超 1 字节）行为正确。

### N3 — Bearer 方案名大小写敏感（downstream/http）

**现状**：`presented_bearer`（`crates/downstream/src/http.rs:34-42`）用 `.strip_prefix("Bearer ")` 抽取
token。RFC 7235/6750 规定鉴权 **scheme** token 大小写不敏感，故客户端合法地发
`Authorization: bearer <key>` 或 `BEARER <key>` 会得到 `None` → 401，即便 key 正确。

**触发与影响**：**fail-safe**（只会拒绝、绝不放行 → 无安全漏洞），但对任何方案名大小写不同的非 rmcp MCP
客户端是真实的互操作坑。

**修复**：把 `presented_bearer` 改为：取 `Authorization` 头字符串，按**首个空格**切分成
`(scheme, rest)`；若 `scheme.eq_ignore_ascii_case("bearer")` 且 `rest` 非空，则返回 `rest` 作为 token。
token 值本身仍原样（大小写敏感）比较——只放宽 scheme 名，不放宽密钥。保持仅一处空格分隔的 RFC 形态，不接受
多余前导空格之外的畸形输入（与现状等价的严格度）。

**测试**：`Bearer k`、`bearer k`、`BEARER k`、`BeArEr k` 均能抽出 `k`；`Basic k`/`Token k` 等其他 scheme →
`None`；`Bearer `（空 token）→ `None`；无 `Authorization` 头 → `None`。`key_authorized` 的常量时间比较保持不变。

## 不做的事（YAGNI）

- 不把 ingest 上限做成可配置（用户已选硬编码常量）。
- 不改 `ingest_tools` 的返回签名/`RebuildSummary`（cap 丢弃只走 warn 遥测，下游本就忽略该返回值）。
- 不放宽 Bearer token 值本身的比较（仍常量时间、大小写敏感）。
- 不引入新依赖。

## 验证

- `cargo fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、
  `cargo test --all-features`、`cargo build --locked` 全绿。
- 新增测试使总数从 189 上升；L1 测试计数块按实际 `cargo test --all-features` 输出更新。
- 同步对应分层文档：config L3/L4（path 校验）、upstream L3/L4（ingest 上限）、downstream L3/L4（Bearer 大小写）。

## 交付

按本仓库一贯工作流：subagent 实现 + 每 task 跑 spec 合规 + 代码质量双重审查、折叠审查 nit、最终整分支
`code-review`、`--no-ff` 本地合并入 master、复测、删除分支、推送 origin。修复后三项 findings 置 `fixed`。
