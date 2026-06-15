# mcpgw 审计整改设计（6 项 Important 发现）

> 状态：已定稿，待 writing-plans 细化为实施计划。
> 来源：本仓库一次全量 subagent 审计（5 个并行 code-review 子代理），共 16 项发现（0 Critical / 6 Important / 10 Minor）。本 spec 只整改 **6 项 Important**；10 项 Minor 暂缓。
> 基线（审计确认健康）：`cargo test --all-features` 162 passed / 3 ignored、clippy 干净、文档 0 死链、L1 测试计数准确、密钥处理稳健、BM25/RRF 数学正确、registry 锁干净、崩溃隔离有效、`CallRecord` 确为仅元数据。

## 目标与范围

修复审计发现的 6 项 Important 问题，每项 TDD（先写失败测试），并同步分层文档。**不**扩展到 Minor 项（如缓存 hash 碰撞、ingest `expect` panic、`isError` 记为 ok、HTTP `with_graceful_shutdown`、reqwest 版本漂移等）——它们单列、暂缓。

不变量：保持纯 Rust、最小依赖（**不新增任何 crate 依赖**）；保持既有 fail-fast 密钥纪律；保持仅元数据审计；保持 `metatools` 纯逻辑。

## 整改项

### F1 — 空 API key 环境变量导致空 Bearer 绕过鉴权（Important，安全；两名审计员独立确认）

**现状**：`resolve_api_keys`（`crates/mcpgw/src/main.rs`）对**缺失**的 env fail-fast，但 `export FOO=`（已设置但为空）会 `Ok("")` 通过；`key_authorized`（`crates/downstream/src/http.rs`）随后 `ct_eq("", b"")==true`，于是 `Authorization: Bearer `（空 token）通过鉴权——而启动日志却报 `auth=true`。

**修复**：
- `resolve_api_keys`：解析出的 secret 若 `trim().is_empty()` → 返回 `Err`（同缺失 env 的 fail-fast 风格，消息只含 key 名/env 名，不含值）。
- 纵深防御：`crates/downstream/src/http.rs` 的 `presented_bearer`/`key_authorized` 把**空的 presented token**视为未授权（在 `ct_eq` 之前直接拒，避免空配置 key 与空 token 互相匹配的任何残留路径）。

**测试**：
- `mcpgw`/`config` 层：api_key 指向的 env 设为 `""` → `resolve_api_keys`/启动返回 `Err`（不 panic、消息不含值）。
- `downstream` http 测试：服务配置了合法非空 key 时，`Authorization: Bearer `（空 token）→ 401；同时回归原有「合法 key 通过 / 错误 key 401」。

### F2 — `{server}__{tool}` 命名空间并非无碰撞（Important，正确性/路由劫持）

**现状**：限定符 `{server}__{tool}` 是下游协议层的工具标识，**必须唯一**；客户端只有这个字符串，结构化 key 无济于事。`config::validate()` 只拒绝**含**字面 `"__"` 的 server 名，但 server 名以 `_` 结尾 + tool 名以 `_` 开头会在边界重组出 `__`：`server="a_"+tool="b"` 与 `server="a"+tool="_b"` 都 → `"a___b"`。tool 名由上游控制，故恶意/异常上游可遮蔽另一上游路由；且 `BTreeMap` last-writer-wins、合并顺序为 `JoinSet` 完成序（不确定）。

**碰撞代数（定稿依据）**：限定符串出现碰撞当且仅当边界出现 ≥3 连续下划线，即**某一 server 名以 `_` 结尾**（tool 侧单独的前导 `_` 在没有「以 `_` 结尾的 server」作配对时无碰撞伙伴）。叠加既有「server 名不含 `__`」规则，**禁止 server 名以 `_` 开头或结尾即可证明使映射单射**。

**修复**：`crates/config/src/lib.rs` 的 `validate()` 上游校验循环里，在现有 blank / `"__"` / 重复检查旁，新增：server 名 `starts_with('_')` 或 `ends_with('_')` → `ConfigError::Invalid`（消息点明规则与该 server 名）。

**测试**：server 名 `"a_"`、`"_a"` 各被拒（`ConfigError::Invalid`）；普通名（`github`、`my_server`）仍通过；既有 `"__"`/blank/重复用例不回归。

### F3 — 未校验 `[server.http].path` 导致启动 panic（Important，健壮性）

**现状**：`config::validate()` 不检查 `server.http.path`；`""`、`"/"`、无前导 `/` 的值会在 `build_router` 的 `axum::Router::nest_service(path, …)`（`crates/downstream/src/http.rs`）里 panic——且发生在监听 bind 成功、打印 `"http server listening"` **之后**，与 `bind` 出错时干净 fail-fast 不对称。

**修复**：`config::validate()` 在 HTTP 段存在时校验 `path`：必须 `starts_with('/')` 且 `len() > 1`（即拒 `""`、`"/"`、无前导斜杠）→ `ConfigError::Invalid`。默认 `"/mcp"` 不受影响。

**测试**：`path = ""` / `"/"` / `"mcp"` 三种各 → `ConfigError::Invalid`；`"/mcp"`（默认）与其它合法路径通过。

### F4 — `CachingEmbedder` 无界增长（Important，资源）

**现状**：`crates/retrieval/src/caching.rs` 的 `HashMap<u64, Arc<[f32]>>` 无淘汰；同一实例横跨所有 rebuild 长期持有，且 vector/hybrid 在**查询**时也走它（`vector.rs:103`），于是每个不同查询串的向量被永久记忆 → 单调增长。

**修复**：把缓存改为**两代有界缓存**（无新依赖）：
- 两个 `HashMap<u64, Arc<[f32]>>`：`current` 与 `previous`，各上限常量 `const CACHE_GEN_CAP: usize`（如 2048）。
- 查找：先查 `current`；未命中再查 `previous`，命中则**晋升**回 `current`（promote-on-hit）。
- 写入：插入 `current`；当 `current.len() >= CACHE_GEN_CAP` 时**轮转**——`previous = take(current)`、`current = 空`（旧 `previous` 整体释放）。
- 内存上界 ~`2 * CACHE_GEN_CAP` 条；频繁访问（如每次 rebuild 命中的 tool 文本）因 promote 保持常驻。
- 保留既有「未命中去重 + Mutex 不跨 `.await` 持有」结构；对外 `Embedder` 行为（返回与输入等长、顺序一致的向量）不变。
- 维持 `u64` 键（Minor 的 hash 碰撞项不在本次范围）。

**测试**（`retrieval` caching）：
- 注入一个计数用的内层 `Embedder`，连续 embed > `2*CAP` 个不同文本后，缓存条目数 `<= 2*CAP`（有界）。
- promote-on-hit：填到刚好触发一次轮转，断言「轮转前访问过的热键」仍能命中（在 current 或经 previous 晋升），且**不再调用内层 embedder**（命中即不回源）。
- 正确性：相同文本始终返回相同向量；与未命中回源结果一致；并发未命中最坏只是重复 embed，不损坏。

### F5 — 计量 `result_bytes`/`arg_bytes` 在热路径重复整体序列化（Important，性能）

**现状**：`crates/downstream/src/lib.rs` 每次调用把整个 response（及 args）`serde_json::to_string(...)` 成一个**用完即弃的 String** 只为取 `.len()`；rmcp 发送时还会再序列化一次 → 大结果被序列化两遍。

**修复**：在 `downstream/src/lib.rs` 加一个私有 `CountingWriter`（`impl std::io::Write`，只累加写入字节数、丢弃内容），用 `serde_json::to_writer(&mut counter, value)` 计字节，避免分配 String。`arg_bytes`/`result_bytes` 改用之。语义（记录的数值）不变。

**测试**：对若干样本，`CountingWriter` 计得的字节数 == 旧 `serde_json::to_string(v).unwrap().len()`；既有「埋点产出仅元数据 CallRecord」测试不回归（`arg_bytes>0`、`result_bytes>0` 等断言仍成立）。

### F6 — `search_tools` 文档写成同步，实际为 `async`（Important，文档准确性）

**现状**：`docs/L4-api/metatools-tools.md:7` 与 `docs/L2-components/metatools.md:33` 把 `search_tools` 写成 `pub fn`，但源码是 `pub async fn`（`crates/metatools/src/tools.rs:8`）——是唯一仍被标成同步的 async 函数。

**修复**：两处签名加 `async`。纯文档改动。

## 分层文档（DoD）

代码 API/行为变化处同步分层文档：
- `docs/L3-details/config.md`（与 L4 `config-lib.md` 如含校验说明）：补 server 名「不得以 `_` 开头/结尾」与 `[server.http].path` 校验规则。
- `docs/L3-details/retrieval.md` / 相关 L4：把缓存描述从「无界、永不淘汰」更新为「两代有界缓存（current+previous，各上限 N，promote-on-hit、满则轮转），内存上界 ~2N」。
- `docs/L4-api/downstream-lib.md` / L3：`result_bytes`/`arg_bytes` 改为经 `CountingWriter` 计量（不再分配 String），数值口径不变。
- F1 鉴权：在 HTTP 鉴权相关文档（L3 mcpgw-cli / downstream http L3/L4）注明「空/空白解析值被拒；空 Bearer 视为未授权」。
- F6 即文档本身。
- 若上述改动影响测试计数，更新 `docs/L1-overview.md` 测试计数块（按 `cargo test --all-features` 实测）。

## 错误处理与不变量（汇总）

- F1/F2/F3 均走既有 `ConfigError`/启动期 `Err`（fail-fast），不 panic、消息不含密钥值。
- F4 缓存改动不改变 `Embedder` 对外契约；仅元数据/纯逻辑不变量不受影响。
- F5 只改计量手段，记录数值与协议响应均不变。
- 不新增任何 crate 依赖；不触碰 Minor 项。

## 实现期需现场确认/可能回退的点

- `CACHE_GEN_CAP` 取值（建议 2048；如需可调再说）；两代实现需保证 Mutex 仍不跨 `.await` 持有（沿用现有「锁内取/插、锁外 await embed」结构）。
- F1 纵深防御点：到底在 `presented_bearer` 返回 `None`（视作未提供）还是在 `key_authorized` 提前拒空——以代码最简且不破坏既有 401 语义为准。
- F2 规则措辞：仅禁「开头/结尾下划线」，不影响中间下划线（`my_server` 合法）。
- F5 `CountingWriter` 对 `Err(response)` 分支仍按现状记 `result_bytes=0`（与现行一致）。
- F3 是否额外拒绝 `path` 含通配符 `*`：以 axum `nest_service` 实际约束为准，最小改动只保证 `starts_with('/') && len>1`。
