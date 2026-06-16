# mcpgw Pass-2 审计修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复第二轮全文审计的 3 项 Minor 发现：补全 `[server.http].path` 校验、给上游 ingest 加体量上限、Bearer 方案名大小写不敏感。

**Architecture:** 三处独立的小改动，各自 TDD（先写失败测试 → 最小实现 → 通过 → 提交）。N1 在 `config::validate()` 增加字符校验；N2 在 `upstream::mapping::ingest_tools` 加两个硬编码上限常量（drop+warn）；N3 重写 `downstream::http::presented_bearer` 的 scheme 解析。最后统一同步分层文档与测试计数，再整分支审查、`--no-ff` 合并。

**Tech Stack:** Rust（cargo workspace）、serde/toml（config）、rmcp 1.7（upstream Tool）、axum 0.8（downstream http）、subtle（常量时间比较，N3 不动它）。

参考 spec：`docs/superpowers/specs/2026-06-16-mcpgw-audit-pass2-design.md`。

**全局验证门禁（每个实现 task 完成后都要过）：**
- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`（注意 clippy `io_other_error`：用 `std::io::Error::other(...)`）
- `cargo test --all-features`（testkit 门控测试需 `--all-features`）
- 提交信息末尾加 `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`

---

## File Structure

| 文件 | 职责 | 本计划改动 |
|---|---|---|
| `crates/config/src/lib.rs` | Config schema + `validate()` + 单测 | N1：`validate()` 增 path 字符校验 + 单测 |
| `crates/upstream/src/mapping.rs` | rmcp `Tool` → `ToolDef`、`ingest_tools` + 单测 | N2：两个上限常量 + `ingest_tools` 加 cap + 单测 |
| `crates/downstream/src/http.rs` | axum 路由 + Bearer 鉴权 + 单测 | N3：`presented_bearer` scheme 大小写不敏感 + 单测 |
| `docs/L3-details/config.md`、`docs/L4-api/config-lib.md` | config 分层文档 | N1：记录 path 字符校验 |
| `docs/L3-details/upstream.md`、`docs/L4-api/upstream-mapping.md` | upstream 分层文档 | N2：记录 ingest 上限 |
| `docs/L3-details/downstream.md`、`docs/L4-api/downstream-http.md` | downstream 分层文档 | N3：记录 Bearer 大小写 |
| `docs/L1-overview.md` | 概览 + 测试计数块 | 末尾按实际 `cargo test --all-features` 更新计数 |

---

## Task 1: N1 — `[server.http].path` 拒绝通配/参数段（config）

**Files:**
- Modify: `crates/config/src/lib.rs:319-326`（`validate()` 内 path 检查），并在同文件 `#[cfg(test)] mod tests` 增测试
- Test: 同文件 `crates/config/src/lib.rs` 的 `mod tests`

- [ ] **Step 1: 写失败测试**

在 `crates/config/src/lib.rs` 的 `mod tests` 内，紧跟现有 `rejects_invalid_http_path` 之后新增两个测试：

```rust
    #[test]
    fn rejects_wildcard_or_param_http_path() {
        for bad in ["/{id}", "/{*rest}", "/a*b", "/x{y}", "/a/{seg}/b"] {
            let toml = format!("[server.http]\nenabled = true\npath = \"{bad}\"\n");
            let err = Config::from_toml_str(&toml).unwrap_err();
            assert!(
                matches!(err, ConfigError::Invalid(_)),
                "http path {bad:?} must be rejected (wildcard/param segment)"
            );
        }
    }

    #[test]
    fn accepts_plain_literal_http_paths() {
        for ok in ["/mcp", "/a/b/c", "/mcp-v1", "/v1.0/mcp", "/gateway_v2"] {
            let toml = format!("[server.http]\nenabled = true\npath = \"{ok}\"\n");
            let cfg = Config::from_toml_str(&toml)
                .unwrap_or_else(|e| panic!("http path {ok:?} must be accepted: {e}"));
            assert_eq!(cfg.server.http.unwrap().path, ok);
        }
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p config rejects_wildcard_or_param_http_path -- --nocapture`
Expected: FAIL —— `/{id}` 等当前能过 `validate()`，断言 `must be rejected` 失败。

- [ ] **Step 3: 最小实现**

把 `crates/config/src/lib.rs` 中现有的 path 检查块（约 319-326 行）：

```rust
        if let Some(http) = &self.server.http {
            if !http.path.starts_with('/') || http.path.len() < 2 {
                return Err(ConfigError::Invalid(format!(
                    "[server.http].path {:?} must start with '/' and be longer than \"/\"",
                    http.path
                )));
            }
        }
```

替换为：

```rust
        if let Some(http) = &self.server.http {
            if !http.path.starts_with('/') || http.path.len() < 2 {
                return Err(ConfigError::Invalid(format!(
                    "[server.http].path {:?} must start with '/' and be longer than \"/\"",
                    http.path
                )));
            }
            if http.path.contains(['{', '}', '*']) {
                return Err(ConfigError::Invalid(format!(
                    "[server.http].path {:?} must not contain wildcard/parameter segments \
                     ('{{', '}}', '*'); use a plain literal path",
                    http.path
                )));
            }
        }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p config http_path`
Expected: PASS —— `rejects_invalid_http_path`、`rejects_wildcard_or_param_http_path`、`accepts_plain_literal_http_paths`、`accepts_default_and_custom_http_path` 全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p config --all-targets -- -D warnings && cargo test -p config
git add crates/config/src/lib.rs
git commit -m "fix(config): reject wildcard/param segments in [server.http].path (audit N1)

A path like \"/{*rest}\" passed validate() but panicked axum at router build;
\"/{id}\" silently mounted MCP at a dynamic capture. Enforce the doc's promise
so the failure is a clean ConfigError::Invalid instead.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: N2 — 给上游 ingest 加体量上限（upstream/mapping）

**Files:**
- Modify: `crates/upstream/src/mapping.rs`（增两个 `pub const` 上限 + 改写 `ingest_tools`），并在同文件 `#[cfg(test)] mod tests` 增测试
- Test: 同文件 `crates/upstream/src/mapping.rs` 的 `mod tests`

背景：`ingest_tools` 当前原样接受上游所有工具，对数量/单工具文本大小无上限（per-ingest 超时只限时间不限体量）。一个被攻陷/异常上游可驱动 catalog/快照内存无界增长。修复用两个硬编码上限 + drop+warn（复用现有 duplicate-skip 风格），返回签名不变（下游 `rebuild_snapshot` 已忽略返回的 dupes 计数）。

- [ ] **Step 1: 写失败测试**

在 `crates/upstream/src/mapping.rs` 的 `mod tests` 内（现有 `ingest_tools_adds_namespaced_and_counts_dupes` 之后）新增。注意 `tool(name, desc)` 辅助构造的工具 `input_schema` 为**空** `JsonObject`，序列化为 `"{}"`（恰 2 字节）：

```rust
    #[test]
    fn ingest_tools_caps_per_server_tool_count() {
        let mut cat = Catalog::new();
        let tools: Vec<_> = (0..(MAX_TOOLS_PER_SERVER + 5))
            .map(|i| tool(&format!("t{i}"), Some("d")))
            .collect();
        let dupes = ingest_tools(&mut cat, "srv", &tools);
        assert_eq!(dupes, 0, "all names unique -> no intra-server dupes");
        assert_eq!(
            cat.len(),
            MAX_TOOLS_PER_SERVER,
            "extras beyond the per-server cap must be dropped"
        );
    }

    #[test]
    fn ingest_tools_skips_a_tool_over_the_text_byte_cap() {
        let mut cat = Catalog::new();
        // empty schema serializes to "{}" (2 bytes); description pushes total over the cap.
        let huge = "a".repeat(MAX_TOOL_TEXT_BYTES + 1);
        let tools = vec![
            tool("small", Some("ok")),
            tool("huge", Some(&huge)),
            tool("also_small", Some("ok2")),
        ];
        let dupes = ingest_tools(&mut cat, "srv", &tools);
        assert_eq!(dupes, 0);
        assert_eq!(cat.len(), 2, "the oversize tool is skipped, others kept");
        assert!(cat.get("srv__small").is_some());
        assert!(cat.get("srv__also_small").is_some());
        assert!(cat.get("srv__huge").is_none(), "oversize tool excluded");
    }

    #[test]
    fn ingest_tools_accepts_a_tool_exactly_at_the_text_byte_cap() {
        let mut cat = Catalog::new();
        // desc bytes + 2 ("{}") == MAX is NOT over the cap (strict `>`), so it is accepted.
        let at_limit = "a".repeat(MAX_TOOL_TEXT_BYTES - 2);
        let tools = vec![tool("edge", Some(&at_limit))];
        ingest_tools(&mut cat, "srv", &tools);
        assert!(
            cat.get("srv__edge").is_some(),
            "a tool exactly at the byte cap must be accepted"
        );
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p upstream ingest_tools_caps -- --nocapture`
Expected: FAIL —— `MAX_TOOLS_PER_SERVER` / `MAX_TOOL_TEXT_BYTES` 未定义（编译错误），且 cap 逻辑未实现。

- [ ] **Step 3: 最小实现**

在 `crates/upstream/src/mapping.rs` 顶部（`tool_to_def` 之前）加两个常量：

```rust
/// Maximum number of tools accepted from a single upstream server per ingest. Extras are
/// dropped (with a warn) to bound catalog/snapshot memory against a compromised upstream.
pub const MAX_TOOLS_PER_SERVER: usize = 1024;

/// Maximum bytes of a single tool's `description` + serialized `input_schema`. A tool over
/// this is skipped (with a warn) so one upstream can't drive unbounded memory/embedding cost.
pub const MAX_TOOL_TEXT_BYTES: usize = 64 * 1024;
```

并把 `ingest_tools` 整体替换为：

```rust
pub fn ingest_tools(catalog: &mut Catalog, server: &str, tools: &[Tool]) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut dupes = 0;
    let mut accepted = 0usize;
    for (i, tool) in tools.iter().enumerate() {
        if accepted >= MAX_TOOLS_PER_SERVER {
            tracing::warn!(
                server,
                dropped = tools.len() - i,
                max = MAX_TOOLS_PER_SERVER,
                "upstream exceeds per-server tool cap; dropping extras"
            );
            break;
        }
        if !seen.insert(tool.name.as_ref()) {
            dupes += 1;
            tracing::warn!(server, tool = %tool.name, "duplicate tool name from upstream; keeping first");
            continue;
        }
        let text_bytes = tool.description.as_deref().unwrap_or("").len()
            + serde_json::to_string(&*tool.input_schema)
                .map(|s| s.len())
                .unwrap_or(0);
        if text_bytes > MAX_TOOL_TEXT_BYTES {
            tracing::warn!(
                server,
                tool = %tool.name,
                bytes = text_bytes,
                max = MAX_TOOL_TEXT_BYTES,
                "tool text exceeds size cap; skipping"
            );
            continue;
        }
        catalog.upsert(tool_to_def(server, tool));
        accepted += 1;
    }
    dupes
}
```

更新 `ingest_tools` 的文档注释（现 17-22 行）末尾补一句：`Tools beyond MAX_TOOLS_PER_SERVER, or whose description+schema exceeds MAX_TOOL_TEXT_BYTES, are dropped with a warn (not counted in the returned dupe count).`

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p upstream`
Expected: PASS —— 三个新测试 + 原有 `ingest_tools_*`/`tool_to_def_*` 全绿。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p upstream --all-targets --all-features -- -D warnings && cargo test -p upstream
git add crates/upstream/src/mapping.rs
git commit -m "fix(upstream): bound per-server tool count and per-tool text size on ingest (audit N2)

A compromised/buggy upstream could return an unbounded tool list (within the
per-ingest timeout), driving unbounded catalog/snapshot memory and embedding
cost. Cap at MAX_TOOLS_PER_SERVER (1024) and MAX_TOOL_TEXT_BYTES (64 KiB),
dropping extras with a warn (reusing the duplicate-skip telemetry style).

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: N3 — Bearer 方案名大小写不敏感（downstream/http）

**Files:**
- Modify: `crates/downstream/src/http.rs:34-42`（`presented_bearer`），并在同文件 `#[cfg(test)] mod tests` 增测试
- Test: 同文件 `crates/downstream/src/http.rs` 的 `mod tests`

背景：`presented_bearer` 用 `.strip_prefix("Bearer ")` 抽 token，对 scheme 名大小写敏感；RFC 7235/6750 规定 scheme 大小写不敏感，故 `bearer`/`BEARER` 即便 key 正确也 401（fail-safe 但互操作坑）。修复只放宽 **scheme 名**，token 值仍原样比较。

- [ ] **Step 1: 写失败测试**

在 `crates/downstream/src/http.rs` 的 `mod tests` 内（现有两个测试之后）新增：

```rust
    #[test]
    fn presented_bearer_scheme_is_case_insensitive() {
        for header in ["Bearer sk-123", "bearer sk-123", "BEARER sk-123", "BeArEr sk-123"] {
            assert_eq!(
                presented_bearer(&req_with_auth(header)),
                Some("sk-123".to_string()),
                "scheme name must be case-insensitive: {header:?}"
            );
        }
    }

    #[test]
    fn presented_bearer_rejects_other_schemes() {
        for header in ["Basic sk-123", "Token sk-123", "sk-123", "Bearersk-123"] {
            assert_eq!(
                presented_bearer(&req_with_auth(header)),
                None,
                "non-bearer scheme must be rejected: {header:?}"
            );
        }
    }

    #[test]
    fn presented_bearer_token_value_stays_case_sensitive() {
        // Only the scheme is case-insensitive; the token itself is returned verbatim.
        assert_eq!(
            presented_bearer(&req_with_auth("bearer SK-Abc")),
            Some("SK-Abc".to_string())
        );
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p downstream presented_bearer_scheme_is_case_insensitive -- --nocapture`
Expected: FAIL —— `bearer sk-123`/`BEARER sk-123` 当前返回 `None`，断言失败。

- [ ] **Step 3: 最小实现**

把 `crates/downstream/src/http.rs` 中现有的 `presented_bearer`：

```rust
fn presented_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}
```

替换为（按首个空格切分 scheme/token，scheme 大小写不敏感，token 非空且原样返回）：

```rust
fn presented_bearer(req: &Request) -> Option<String> {
    let value = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token.to_string())
}
```

并把现有 `presented_bearer_treats_empty_token_as_absent` 测试里的注释（提到 `strip_prefix("Bearer ")`）更新为新实现的措辞，例如：`// "Bearer " splits into scheme="Bearer", token="" -> empty token is treated as not presented.`

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p downstream presented_bearer`
Expected: PASS —— 5 个 `presented_bearer_*` 测试全绿（含原有两个）。

- [ ] **Step 5: 全门禁 + 提交**

Run:
```bash
cargo fmt --all --check && cargo clippy -p downstream --all-targets --all-features -- -D warnings && cargo test -p downstream
git add crates/downstream/src/http.rs
git commit -m "fix(downstream): match Bearer auth scheme case-insensitively (audit N3)

RFC 7235 scheme tokens are case-insensitive; strip_prefix(\"Bearer \") rejected
a valid \"bearer <key>\"/\"BEARER <key>\" with a 401. Compare the scheme with
eq_ignore_ascii_case; the token value itself stays case-sensitive.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: 分层文档同步（docs，无代码改动）

**Files:**
- Modify: `docs/L4-api/config-lib.md`、`docs/L3-details/config.md`（N1 path 校验）
- Modify: `docs/L4-api/upstream-mapping.md`、`docs/L3-details/upstream.md`（N2 ingest 上限）
- Modify: `docs/L4-api/downstream-http.md`、`docs/L3-details/downstream.md`（N3 Bearer 大小写）
- Modify: `docs/L1-overview.md`（测试计数块）

文档为**中文**（含英文代码标识符），匹配各文件既有风格。先读每个目标段落再改，确保如实描述已落地代码。

- [ ] **Step 1: N1 — config 文档**

在 `docs/L4-api/config-lib.md` 的 path 校验描述处（约 28 行、并核对 90 行 `HttpConfig.path` 注释）补充：`[server.http].path` 现还**拒绝含 `{` / `}` / `*` 的通配/参数段**（否则会让 axum 在 router 构建期 panic 或静默错挂），失败为干净的 `ConfigError::Invalid`。
在 `docs/L3-details/config.md` 对应 `[server.http]` / path 段落补同一条说明。

- [ ] **Step 2: N2 — upstream 文档**

在 `docs/L4-api/upstream-mapping.md` 的 `ingest_tools` 段（约 20-34 行）补充：除 intra-server 去重外，`ingest_tools` 现对**单上游工具数**（`MAX_TOOLS_PER_SERVER = 1024`）与**单工具 `description`+序列化 `input_schema` 字节数**（`MAX_TOOL_TEXT_BYTES = 64 KiB`）设上限，超限工具 **drop + warn**（不计入返回的 dupe 数），用于对半可信上游设防内存无界增长。
在 `docs/L3-details/upstream.md` 的 ingest 去重段（约 71-83 行）补同一条，并点明 per-ingest 超时只限时间、这两个上限补上「限体量」的缺口。

- [ ] **Step 3: N3 — downstream 文档**

在 `docs/L4-api/downstream-http.md`（约 33-35 行）与 `docs/L3-details/downstream.md` 的 Bearer 提取段（约 7-9 行）把对 `strip_prefix("Bearer ")` 的描述更新为：`presented_bearer` 现按首个空格切分，**scheme 名大小写不敏感**（`eq_ignore_ascii_case("bearer")`，故 `bearer`/`BEARER` 同样接受），token 值仍原样（大小写敏感）且空 token 仍视为「未呈现」→ 401。

- [ ] **Step 4: 重算并更新 L1 测试计数**

Run: `cargo test --all-features 2>&1 | grep "test result:"`
把每个套件的 passed 求和，更新 `docs/L1-overview.md` 测试计数块（约 322-329 行）的总数与对应分项：本计划新增 config +2、upstream +3、downstream(http) +3，故 189 → **197**（除非实际数不同，以 `cargo test --all-features` 实跑为准）。3 ignored 不变。不要改无关分项。

- [ ] **Step 5: 校对 + 提交**

逐条核对每处文档与真实代码一致（path 字符校验、ingest 两上限常量与单位、Bearer scheme 大小写）。

Run:
```bash
git add docs/
git commit -m "docs: sync layered docs for audit N1/N2/N3 + test count

- config L3/L4: [server.http].path now rejects wildcard/param segments.
- upstream L3/L4: ingest_tools caps per-server tool count + per-tool text bytes.
- downstream L3/L4: Bearer scheme matched case-insensitively.
- L1: recount cargo test --all-features.

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: 验证 + 最终审查 + 合并

**Files:** 无代码改动（验证与集成）。

- [ ] **Step 1: 全门禁复跑**

Run:
```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features 2>&1 | grep "test result:"
cargo build --locked
```
Expected: fmt 干净、clippy 零告警、测试全绿（总数 = L1 所记）、build 成功。

- [ ] **Step 2: 整分支最终 code-review**

以 `git merge-base master <branch>` 为基，对整条分支 diff 跑一次 `code-review` 子代理（model `claude-opus-4.8`）。只折叠 Critical/Important 项；Minor 视情况。

- [ ] **Step 3: 合并（finishing-a-development-branch）**

征得用户确认后，`--no-ff` 本地合并入 master、在 master 复跑 `cargo test --all-features` 确认全绿、删除分支。

- [ ] **Step 4: 推送 + 收尾**

`git push origin master`。把 findings 表 id 17/18/19 置 `status='fixed'`。向用户用中文汇报完成。

---

## Self-Review（plan 作者自查）

- **Spec coverage**：N1 → Task 1；N2 → Task 2；N3 → Task 3；文档同步 + L1 计数 → Task 4；验证/审查/合并/推送 → Task 5。spec「不做的事」均未越界（无可配置、不改返回签名、不放宽 token 比较、无新依赖）。✓
- **Placeholder scan**：每个代码步骤均含完整代码与确切命令；无 TBD/TODO。✓
- **Type/名一致**：常量名 `MAX_TOOLS_PER_SERVER` / `MAX_TOOL_TEXT_BYTES`、函数 `presented_bearer` / `ingest_tools` / `validate` 全程一致；测试辅助 `tool(name, desc)` / `req_with_auth(value)` 与现有代码一致。✓

