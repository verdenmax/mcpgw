# mcpgw-visual-guide 设计（图解教程子项目）

- 状态：已批准（brainstorming）
- 日期：2026-06-13
- 关联：mcpgw 项目本身（`crates/*`）、分层文档 `docs/L1-overview.md` … `docs/L4-api/*`
- 参考样板：`../langchain-visual-guide`（无依赖 Python 生成器产出自包含 HTML 图解站点）

## 1. 目标

在 mcpgw 仓库根下新建子目录 `mcpgw-visual-guide/`，做一套**面向读者**的可视化图解教程
（中文、自包含 HTML），仿照 `langchain-visual-guide` 的结构与设计系统：既有**宏观全景**，
也有**内部源码**讲解，每课对照 mcpgw 真实源码（crate 文件 + 符号名，**不写死行号**，避免随上游漂移）。

**首版（本次迭代）写透两部分**：① 宏观全景（是什么 / 架构 / 调用生命周期）；② 向量检索专章。
其余（各 crate 内部、Hybrid）**注册为「施工中」占位页，并作为可点击的导航项出现**，后续逐步补全。

教程是**独立于** `docs/L1–L4`（参考型分层文档）的**教学型**产物，可从 L1–L4 取材但面向新读者重新组织。

## 2. 非目标（YAGNI / 延后）

- PDF（`print.html` → 无头 Chrome 打印）——延后到内容稳定后。
- GitHub Pages / CI（deploy + 防回归 + 死链检查）——延后。
- quizzes（测验）、glossary（术语表）——延后。
- 任何第三方依赖：生成器仅用 Python 3 标准库；站点为纯静态 HTML/CSS，无 JS 框架。

## 3. 目录与生成器架构

```
mcpgw-visual-guide/
├── index.html              ← 生成产物（目录页 / 入口）
├── lessons/                ← 生成产物
│   └── NN-*.html
├── src/                    ← 无依赖 Python 生成器（可重建全部 HTML）
│   ├── shell.py            共享外壳：CSS 设计系统、head_meta()、page()、index_page()、
│   │                       PAGES（有序课程清单）、PARTS（分部）、上一/下一课导航、INDEX_FILE
│   ├── part1_macro.py      第一部分 · 宏观全景（3 课，写透）：LESSON_01..03
│   ├── part2_vector.py     第二部分 · 向量检索（5 课，写透）：LESSON_04..08
│   ├── part3_internals.py  第三部分 · 各 crate 内部（6 课，占位）：LESSON_09..14
│   ├── part4_next.py       第四部分 · 后续（1 课，占位）：LESSON_15（Hybrid/RRF）
│   ├── registry.py         单一事实源：filename -> 课程 HTML 内容 的有序映射
│   └── build.py            站点构建：写 index.html + lessons/NN-*.html
├── README.md               子项目说明（如何阅读 / 重新生成 / 结构 / 后续计划）
└── .gitignore              src/__pycache__/ 等
```

- 内容**作者方式与样板一致**：每课是 `LESSON_XX = r"""<原始 HTML>"""`，直接使用 `shell.py`
  设计系统里的 CSS 类（卡片、代码标注、折叠、流程图、表格等）；不引入内容辅助函数层（保持与样板一致、低复杂度）。
- 占位页用统一的「施工中」内容模板（一段说明 + 该课将覆盖的要点清单 + 指向已写透课程/对应源码的链接），
  确保占位页**也是合法、可导航、信息有用**的页面，而非空白。
- `build.py` 遍历 `shell.PAGES`，对每个 filename 取 `registry.CONTENT[filename]`，用 `shell.page(...)`
  包壳写入 `lessons/`；再写 `index.html`（`shell.index_page(...)`）。页面间用**相对链接**（`file://` 可直接打开）。

## 4. 设计系统（移植自样板）

复用样板的视觉语言（同一套 CSS 收进 `shell.py`）：

- 顶部 **sticky 进度条** + 左上 home pill + 右上「第 N 部分 · 第 M 课」pill。
- **hero**：part 小标 + h1 + lead 引言。
- **卡片体系**（`.card` + 修饰类）：🌍 宏观理解 `macro`、🔬 细节/代码对应 `detail`、🔌 生活类比 `analogy`、
  ✅ 关键要点 `key`、💡 设计亮点 `spark`、⚠️ 注意 `warn`。
- **代码文件标注** `.codefile`：头部显示 `crate/src/file.rs` 路径 + 符号名；体内是高亮代码片段。
- **流程图** `.flow`、**折叠** `.accordion`（summary + 编号徽标）、**表格** `.t`、行内代码 `.inline`/`.mono`。
- **上一课 / 下一课** 导航 + 返回首页；**深色模式**（`prefers-color-scheme`）。

## 5. 内容大纲（章节规划）

| # | 文件名 | 课程标题 | 状态 |
|---|--------|----------|------|
| **第一部分 · 宏观全景** ||||
| 1 | `01-what-is-mcpgw.html` | mcpgw 是什么 — 工具爆炸问题 · 3 元工具渐进式发现心智模型 | ✍️ 写透 |
| 2 | `02-architecture.html` | 整体架构全景 — Cargo workspace 各 crate · 依赖方向 · stdio/HTTP 传输能力 | ✍️ 写透 |
| 3 | `03-call-lifecycle.html` | 一次工具调用的生命周期 — `search_tools` → `get_tool_details` → `call_tool` 数据流 | ✍️ 写透 |
| **第二部分 · 检索深入 · 向量检索** ||||
| 4 | `04-vector-overview.html` | 向量检索总览 — 为何需要语义检索 · 与 BM25 对比（漏召回示例）· 可插拔 + 透明降级设计 | ✍️ 写透 |
| 5 | `05-embedder.html` | Embedder 抽象 & OpenAiEmbedder — `Embedder`/`EmbedError` · 独立 `embedder` crate · 密钥来自 env | ✍️ 写透 |
| 6 | `06-caching-embedder.html` | CachingEmbedder — 内容哈希（FNV-1a）缓存 · 跨 rebuild 持久 · 不跨 `.await` 持锁 | ✍️ 写透 |
| 7 | `07-vector-strategy.html` | VectorStrategy — 余弦 + 内置 BM25 双重降级 · 索引/检索流程 · 零范数防 NaN | ✍️ 写透 |
| 8 | `08-wiring-config.html` | 装配与配置 — `build_strategy`/`build_embedder` · `[retrieval.vector]` · 启动期 fail-fast · 默认仍 bm25 | ✍️ 写透 |
| **第三部分 · 各 crate 内部** ||||
| 9 | `09-catalog.html` | catalog — 工具目录与命名空间 | 🚧 占位 |
| 10 | `10-upstream.html` | upstream — 活上游 I/O · 连接/摄取/路由 · 降级启动 | 🚧 占位 |
| 11 | `11-gateway-metatools.html` | gateway + metatools — ArcSwap 快照状态 · rebuild · 3 元工具 | 🚧 占位 |
| 12 | `12-downstream.html` | downstream — rmcp ServerHandler · stdio + HTTP 传输 · Bearer 鉴权 | 🚧 占位 |
| 13 | `13-retrieval-bm25.html` | retrieval 内核 — `RetrievalStrategy`(async) 与 BM25 | 🚧 占位 |
| 14 | `14-config.html` | config — 配置系统（`[server]`/`[upstream]`/`[retrieval]`） | 🚧 占位 |
| **第四部分 · 后续** ||||
| 15 | `15-hybrid-rrf.html` | Hybrid 检索（RRF）— 待 M2-B 实现后补全 | 🚧 占位 |

> 占位页（9–15）均为可点击导航项，内含「施工中」说明 + 计划要点 + 指向相关写透课程/源码的链接。

## 6. 向量检索专章 ↔ 真实源码锚点

| 课 | 主要源码锚点（文件 + 符号） |
|----|------------------------------|
| 4 向量总览 | `crates/retrieval/src/lib.rs`（`RetrievalStrategy` async trait、`build_strategy`）、`vector.rs` 概念 |
| 5 Embedder | `crates/retrieval/src/embedder.rs`（`Embedder`/`EmbedError`/`MockEmbedder`）、`crates/embedder/src/lib.rs`（`OpenAiEmbedder`） |
| 6 CachingEmbedder | `crates/retrieval/src/caching.rs`（`CachingEmbedder`、FNV-1a、锁纪律、insert-only 不变量） |
| 7 VectorStrategy | `crates/retrieval/src/vector.rs`（`VectorStrategy`、`index`/`search`、`normalize` 零范数防护、双重降级、count-mismatch 降级） |
| 8 装配与配置 | `crates/retrieval/src/lib.rs`（`build_strategy(name, embedder)`、`StrategyError::EmbedderRequired`）、`crates/mcpgw/src/main.rs`（`build_embedder`、`prepare_state`）、`crates/config/src/lib.rs`（`VectorConfig`、`validate`、`batch_size` 为预留） |

内容须与已合并的 M2-A 代码及 L1–L4 文档**保持一致**：检索异步可插拔、透明 BM25 降级、`batch_size` 当前未启用（预留）、
默认策略仍 `bm25`、`strategy = "vector"` 仅在 `serve` 下生效（离线 `search`/`get-details` CLI 不注入 embedder）。

## 7. 构建与验证

- 重新生成：`cd mcpgw-visual-guide/src && python build.py`。
- 本地预览：`cd mcpgw-visual-guide && python -m http.server 8000` → `http://localhost:8000/`。
- 验证（生成器自带、无第三方依赖）：
  - `build.py` 跑通且无报错；产出 `index.html` 与 `lessons/01..15-*.html` 共 16 个文件。
  - 内部相对链接无死链（首版可用一个简易 stdlib 脚本或人工核对；正式 `check_links.py` 随 CI 延后）。
  - 每页 HTML 结构完整（有 hero、导航、进度条）；首页 TOC 列全 15 课且链接正确。
- 提交：`mcpgw-visual-guide/` 作为 mcpgw 仓库的子目录提交（首版不单独成库；未来可拆出独立仓库 + Pages）。

## 8. 验收标准（首版完成的定义）

1. `mcpgw-visual-guide/` 目录与上述生成器骨架建成；`python build.py` 成功产出 `index.html` + 15 课。
2. 第一部分（1–3）与第二部分（4–8）共 **8 课写透**，内容对照真实源码、与 L1–L4 一致、无事实错误。
3. 第三/四部分（9–15）共 **7 个占位页**生成且**作为可点击导航项**出现在 TOC 与上一/下一课链中，内容为有用的「施工中」模板。
4. 站点可经 `file://` 直接打开与静态服务器预览；内部链接无死链；深色模式与导航工作正常。
5. README 说明如何阅读 / 重新生成 / 目录结构 / 后续计划（PDF、Pages/CI、其余 crate、Hybrid）。

## 9. 后续（不在首版范围）

- 写透第三部分各 crate 内部（9–14）。
- M2-B 完成后写透 Hybrid/RRF（15）。
- 加 `build_print.py` → PDF；加 GitHub Pages + CI（deploy / 防漂移 / 死链）；可选 quizzes、glossary。
