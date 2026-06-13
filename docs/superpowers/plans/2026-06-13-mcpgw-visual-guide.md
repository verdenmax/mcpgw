# mcpgw-visual-guide 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 mcpgw 仓库根下新建 `mcpgw-visual-guide/` 子项目：一套无依赖 Python 生成器产出的自包含 HTML 图解教程，首版写透「宏观全景（3 课）」+「向量检索专章（5 课）」，其余 7 课注册为可导航的「施工中」占位页。

**Architecture:** 完全仿照 `../langchain-visual-guide`：`src/shell.py` 持有 CSS 设计系统 + `page()`/`index_page()` + `PAGES`（有序课程清单，分部标签内嵌在每项元组里）+ 导航；`src/partN_*.py` 以 `LESSON_XX = r"""<原始 HTML>"""` 形式写课程内容；`src/registry.py` 映射 filename→内容；`src/build.py` 遍历 `PAGES` 写出 `index.html` + `lessons/NN-*.html`。页面用相对链接，`file://` 可直接打开。

**Tech Stack:** Python 3 标准库（仅生成器）；纯静态 HTML/CSS，无 JS 框架、无第三方依赖。内容对照已合并的 mcpgw M2-A 真实源码（crate 文件 + 符号名，不写死行号）。

**参考样板：** `/home/verden/course/langchain-visual-guide/src/{shell.py,build.py,registry.py,part1.py}`。

**Spec:** `docs/superpowers/specs/2026-06-13-mcpgw-visual-guide-design.md`。

---

## 文件结构

生成器源码（手写）：
- `mcpgw-visual-guide/src/shell.py` — CSS 设计系统 + `head_meta()` + `PAGES`(有序课程清单) + `page()` + `index_page()` + `INDEX_FILE`。移植自样板，文案改为 mcpgw。
- `mcpgw-visual-guide/src/part1_macro.py` — `LESSON_01/02/03`（宏观全景，写透）。
- `mcpgw-visual-guide/src/part2_vector.py` — `LESSON_04/05/06/07/08`（向量检索，写透）。
- `mcpgw-visual-guide/src/part3_internals.py` — `LESSON_09..14`（各 crate 内部，占位）。
- `mcpgw-visual-guide/src/part4_next.py` — `LESSON_15`（Hybrid/RRF，占位）。
- `mcpgw-visual-guide/src/placeholder.py` — `build(title, points, links)` 生成统一「施工中」占位页 HTML 的小工具。
- `mcpgw-visual-guide/src/registry.py` — `CONTENT`：filename→课程 HTML 的有序映射。
- `mcpgw-visual-guide/src/build.py` — 站点构建入口。
- `mcpgw-visual-guide/src/check_links.py` — stdlib-only 内部死链检查（验证用）。
- `mcpgw-visual-guide/README.md` — 子项目说明。
- `mcpgw-visual-guide/.gitignore` — 忽略 `src/__pycache__/`。

生成产物（由 `build.py` 写出，纳入 git）：
- `mcpgw-visual-guide/index.html`
- `mcpgw-visual-guide/lessons/01-*.html` … `15-*.html`（共 15 课）。

---

## Task 1: 生成器骨架（shell.py 设计系统 + PAGES + page/index_page）

建立无依赖 Python 生成器的核心外壳：移植样板 `shell.py` 的 CSS 设计系统与页面/索引包壳函数，但 `PAGES` 与所有文案改成 mcpgw 的 15 课。本任务先用一个**最小的占位内容**让 `build.py` 跑通并产出全部页面骨架；真实课程内容在后续任务填充。

**Files:**
- Create: `mcpgw-visual-guide/src/shell.py`
- Create: `mcpgw-visual-guide/src/registry.py`
- Create: `mcpgw-visual-guide/src/build.py`
- Create: `mcpgw-visual-guide/.gitignore`

- [ ] **Step 1: 写 shell.py（设计系统 + 导航外壳）**

完整移植样板 `/home/verden/course/langchain-visual-guide/src/shell.py` 的以下部分，逐字保留，仅按标注改文案：
1. `import base64` 与 favicon（把 favicon 的字母 `λ` 改为 `M`，方块色保留 `#1a7f64`）。
2. `head_meta(title, description, og_type)` —— 把 `og:site_name`/twitter 文案里的「LangChain 图解教程」改为「mcpgw 图解教程」。
3. `CSS = r"""…"""` —— **逐字照抄**样板 `shell.py` 第 66–304 行的整段 CSS（设计 token、topbar/progress、hero、卡片 `.card.{macro,detail,analogy,key,warn,spark}`、`.codefile`、`pre.code`、`.accordion`、`.flow`/`.vflow`、`.layers`/`.layer.{l-core,l-main,l-part,l-app}`、`.cols`、`table.t`、`.selftest`/`.quiz`、`.footnav`、index 页 `.toc`/`.toc-search`/`.legend`/`.pdf-btn`）。不改任何 CSS。
4. `SEARCH_JS`、`NAV_SCRIPT` —— 逐字照抄。
5. `INDEX_FILE = "index.html"`。
6. `PAGES` —— 替换为 mcpgw 的 15 课（见下方代码）。
7. `page(filename, content, standalone=False, home_href=None)` —— 逐字照抄样板实现，仅把 `<title>` 与 topbar 文案中的「LangChain 图解教程」改成「mcpgw 图解教程」。
8. `index_page(standalone=False, lesson_prefix="")` —— 逐字照抄样板实现，但：(a) `subtitles` 字典换成 mcpgw 15 课的副标题（见下）；(b) hero 的 h1/lead/part 文案改 mcpgw（见下）；(c) **删除** PDF 下载按钮那段 `<a ... class="pdf-btn">`（PDF 延后，避免死链）；(d) 顶部「📘 LangChain 图解教程」改「📘 mcpgw 图解教程」；(e) 底部版本锚点行改为对照 mcpgw `master`（M2-A 合并后）、核验 2026-06。

`PAGES`（替换样板同名变量）：

```python
# Ordered list of all pages: (filename, short title, part label)
PAGES = [
    ("01-what-is-mcpgw.html", "mcpgw 是什么", "第一部分 · 宏观全景"),
    ("02-architecture.html", "整体架构全景", "第一部分 · 宏观全景"),
    ("03-call-lifecycle.html", "一次工具调用的生命周期", "第一部分 · 宏观全景"),
    ("04-vector-overview.html", "向量检索总览", "第二部分 · 检索深入 · 向量检索"),
    ("05-embedder.html", "Embedder 抽象 & OpenAiEmbedder", "第二部分 · 检索深入 · 向量检索"),
    ("06-caching-embedder.html", "CachingEmbedder 缓存", "第二部分 · 检索深入 · 向量检索"),
    ("07-vector-strategy.html", "VectorStrategy 余弦 + 降级", "第二部分 · 检索深入 · 向量检索"),
    ("08-wiring-config.html", "装配与配置", "第二部分 · 检索深入 · 向量检索"),
    ("09-catalog.html", "catalog 工具目录", "第三部分 · 各 crate 内部"),
    ("10-upstream.html", "upstream 上游 I/O", "第三部分 · 各 crate 内部"),
    ("11-gateway-metatools.html", "gateway + metatools", "第三部分 · 各 crate 内部"),
    ("12-downstream.html", "downstream 下游服务", "第三部分 · 各 crate 内部"),
    ("13-retrieval-bm25.html", "retrieval 内核与 BM25", "第三部分 · 各 crate 内部"),
    ("14-config.html", "config 配置系统", "第三部分 · 各 crate 内部"),
    ("15-hybrid-rrf.html", "Hybrid 检索（RRF）", "第四部分 · 后续"),
]
```

`index_page` 里的 `subtitles`（替换样板同名字典）：

```python
    subtitles = {
        "01-what-is-mcpgw.html": "工具爆炸 · 3 元工具渐进式发现",
        "02-architecture.html": "workspace 各 crate · 依赖方向 · 传输",
        "03-call-lifecycle.html": "search → get_details → call 数据流",
        "04-vector-overview.html": "为何语义检索 · 对比 BM25 · 透明降级",
        "05-embedder.html": "Embedder/EmbedError · 独立 crate · env 密钥",
        "06-caching-embedder.html": "内容哈希缓存 · 跨 rebuild 持久 · 锁纪律",
        "07-vector-strategy.html": "余弦 + 内置 BM25 双重降级 · 防 NaN",
        "08-wiring-config.html": "build_strategy/embedder · [retrieval.vector]",
        "09-catalog.html": "工具目录与命名空间（施工中）",
        "10-upstream.html": "活上游 I/O · 连接/摄取/路由（施工中）",
        "11-gateway-metatools.html": "ArcSwap 快照 · 3 元工具（施工中）",
        "12-downstream.html": "rmcp ServerHandler · stdio + HTTP（施工中）",
        "13-retrieval-bm25.html": "RetrievalStrategy(async) 与 BM25（施工中）",
        "14-config.html": "[server]/[upstream]/[retrieval] 配置（施工中）",
        "15-hybrid-rrf.html": "RRF 融合 BM25+向量（待 M2-B）",
    }
```

`index_page` 的 hero 文案（替换样板 hero index 段内的对应文本）：

```html
  <div class="hero index">
    <div class="part">从零开始 · 面向 mcpgw 读者</div>
    <h1>用图解理解整个 mcpgw 网关项目</h1>
    <p class="lead">这套教程带你<strong>层层深入</strong>：先建立 mcpgw 的<strong>宏观全景</strong>（是什么 / 架构 / 一次调用的生命周期），
    再写透<strong>向量检索专章</strong>（Embedder / 缓存 / VectorStrategy / 装配配置）。其余各 crate 内部与 Hybrid 检索为<strong>施工中</strong>占位，将逐步补全。
    每一课都对照 mcpgw 真实源码（crate 文件 + 符号名）。</p>
    <div class="legend">
      <span><i style="background:var(--blue)"></i>宏观理解</span>
      <span><i style="background:var(--purple)"></i>细节 / 源码</span>
      <span><i style="background:var(--amber)"></i>生活类比</span>
      <span><i style="background:var(--accent)"></i>关键要点</span>
    </div>
    <p style="margin:1rem 0 0;color:var(--faint);font-size:.8rem">📌 对照 mcpgw <strong>master</strong>（M2-A 向量检索合并后）· 最后核验 2026-06 · 源码引用以"文件 + 符号名"为主（行号会随代码更新而变）</p>
  </div>
```

- [ ] **Step 2: 写 registry.py（先全部指向最小占位，保证可构建）**

本步先让每课内容都是一行最小占位，使 `build.py` 能立即跑通；真实内容由后续任务替换映射值。

```python
"""Single source of truth: ordered map of output filename -> lesson HTML content.

build.py imports this so the lesson set stays in sync with shell.PAGES.
"""
import shell

# Filename -> lesson HTML. Real content is filled in by later tasks; until then
# every page renders a one-line stub so the site builds end-to-end.
CONTENT = {fname: f"<p>（待填充：{title}）</p>" for fname, title, _part in shell.PAGES}
```

- [ ] **Step 3: 写 build.py（站点构建入口）**

```python
"""Build the mcpgw visual guide as a standalone static site.

    index.html           entry point (table of contents)
    lessons/NN-*.html    lesson pages

Pages use relative links so the site works via file:// or any static server.

Usage:
    cd mcpgw-visual-guide/src && python build.py
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))  # project root
LESSONS_DIR = os.path.join(ROOT, "lessons")
sys.path.insert(0, HERE)

import shell  # noqa: E402
from registry import CONTENT  # noqa: E402


def build():
    os.makedirs(LESSONS_DIR, exist_ok=True)
    written = []
    for fname, _title, _part in shell.PAGES:
        html = shell.page(fname, CONTENT[fname], standalone=True, home_href="../index.html")
        with open(os.path.join(LESSONS_DIR, fname), "w", encoding="utf-8") as f:
            f.write(html)
        written.append(os.path.join("lessons", fname))
    with open(os.path.join(ROOT, shell.INDEX_FILE), "w", encoding="utf-8") as f:
        f.write(shell.index_page(standalone=True, lesson_prefix="lessons/"))
    written.append(shell.INDEX_FILE)
    return written


if __name__ == "__main__":
    done = build()
    print("Wrote", len(done), "files under", ROOT)
    for f in done:
        print("  -", f)
```

- [ ] **Step 4: 写 .gitignore**

`mcpgw-visual-guide/.gitignore`：

```
src/__pycache__/
__pycache__/
*.pyc
```

- [ ] **Step 5: 运行构建，验证产出 16 个文件**

Run: `cd mcpgw-visual-guide/src && python build.py`
Expected: 打印 `Wrote 16 files under …`，列出 `index.html` 与 `lessons/01-*.html`…`15-*.html`（15 课 + 1 首页 = 16）。无报错。

- [ ] **Step 6: 冒烟校验首页与一课**

Run: `cd mcpgw-visual-guide && python - <<'PY'
import re, pathlib
idx = pathlib.Path("index.html").read_text(encoding="utf-8")
assert "mcpgw 图解教程" in idx, "首页标题文案缺失"
assert idx.count('class="n"') == 15, f"TOC 应有 15 课，实际 {idx.count(chr(34)+chr(110)+chr(34))}"
for f in ["01-what-is-mcpgw.html","08-wiring-config.html","15-hybrid-rrf.html"]:
    h = pathlib.Path("lessons", f).read_text(encoding="utf-8")
    assert "<div class=\"footnav\">" in h, f"{f} 缺导航"
    assert "progress" in h, f"{f} 缺进度条"
print("SMOKE OK")
PY`
Expected: 打印 `SMOKE OK`。

- [ ] **Step 7: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/shell.py mcpgw-visual-guide/src/registry.py mcpgw-visual-guide/src/build.py mcpgw-visual-guide/.gitignore mcpgw-visual-guide/index.html mcpgw-visual-guide/lessons
git commit -m "feat(guide): scaffold mcpgw-visual-guide generator (shell + build, stub content)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: 占位页工具 + 7 个「施工中」可导航页

实现统一的「施工中」占位页模板，并把第三/四部分的 7 课（09–15）接到 `registry.py`，让它们成为**内容有用、可点击**的导航页（spec 验收 3）。占位页含：一句说明 + 该课计划要点清单 + 指向已写透课程/对应源码的链接。

**Files:**
- Create: `mcpgw-visual-guide/src/placeholder.py`
- Create: `mcpgw-visual-guide/src/part3_internals.py`
- Create: `mcpgw-visual-guide/src/part4_next.py`
- Modify: `mcpgw-visual-guide/src/registry.py`

- [ ] **Step 1: 写 placeholder.py（占位页 HTML 生成器）**

```python
"""Shared 'under construction' page template for not-yet-written lessons.

build(intro, points, links) returns lesson-body HTML using the shell design
system (warn card + key card + bullet list). Pages stay valid and navigable.
"""


def build(intro, points, links):
    """intro: str (one-paragraph summary).
    points: list[str] — planned coverage bullets (HTML allowed).
    links: list[(href, label)] — related written lessons / source anchors.
    """
    pts = "\n".join(f"<li>{p}</li>" for p in points)
    lnks = "\n".join(
        f'<li><a href="{href}">{label}</a></li>' for href, label in links
    )
    return f"""
<div class="card warn">
  <div class="tag">🚧 施工中</div>
  本课尚未写透，目前是占位页。{intro}
</div>

<h2>本课将覆盖</h2>
<ul>
{pts}
</ul>

<div class="card key">
  <div class="tag">✅ 先看这些</div>
  在本课补全前，可先阅读相关已写透课程与对应源码：
  <ul>
{lnks}
  </ul>
</div>
"""
```

- [ ] **Step 2: 写 part3_internals.py（09–14 占位）**

每课调用 `placeholder.build(...)`。链接用**同目录裸文件名**（lesson 页之间相对链接），指向最相关的已写透课程。

```python
"""Part 3 (per-crate internals) lessons — placeholders for now."""
import placeholder

LESSON_09 = placeholder.build(
    "catalog 是工具目录与命名空间层：把 N 个上游的工具收进 `{server}__{name}` 命名空间，供检索与路由使用。",
    [
        "<code>ToolDef</code> / <code>Catalog</code> 数据结构与命名空间（<code>crates/catalog/src/lib.rs</code>）",
        "<code>from_json_str</code> / <code>from_tooldefs</code> 两种装载路径",
        "<code>qualified_name = \"{server}__{name}\"</code> 规则与冲突检测",
    ],
    [
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（catalog 如何被检索/路由）"),
        ("13-retrieval-bm25.html", "13 · retrieval 内核与 BM25"),
    ],
)

LESSON_10 = placeholder.build(
    "upstream 是活的上游 MCP I/O 层：rmcp client 连接、工具摄取、call_tool 转发，支持 stdio 子进程与远程 HTTP，连不上则降级隔离。",
    [
        "<code>UpstreamHandle</code> / <code>UpstreamRegistry</code>（<code>crates/upstream/src/</code>）",
        "<code>connect_all</code> eager-connect、env allow-list、握手超时",
        "stdio 子进程 vs 远程 HTTP（<code>bearer_env</code> 原始 token、<code>headers</code> 头名→env）",
    ],
    [
        ("02-architecture.html", "02 · 整体架构全景（上游在哪一层）"),
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（call_tool 路由回上游）"),
    ],
)

LESSON_11 = placeholder.build(
    "gateway 持有 ArcSwap 快照状态并做 build-then-swap 重建；metatools 在不可变快照上提供 search_tools / get_tool_details / call_tool 三个元工具。",
    [
        "<code>GatewayState</code>：<code>ArcSwap&lt;GatewaySnapshot&gt;</code>（读无锁）+ rebuild 串行化（<code>crates/gateway/src/lib.rs</code>）",
        "<code>rebuild_snapshot</code>：并发摄取 + per-ingest 超时 + 原子 swap",
        "三个元工具函数与 <code>call_tool</code> 经 catalog 路由（<code>crates/metatools/src/</code>）",
    ],
    [
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（元工具数据流）"),
        ("07-vector-strategy.html", "07 · VectorStrategy（快照里装的检索策略）"),
        ("08-wiring-config.html", "08 · 装配与配置（embedder 如何注入 GatewayState）"),
    ],
)

LESSON_12 = placeholder.build(
    "downstream 把 3 个元工具暴露为真正的 MCP 服务：rmcp ServerHandler（stdio）+ Streamable HTTP（axum，多 key 常量时间 Bearer 鉴权）。",
    [
        "<code>GatewayServer</code> 实现 rmcp <code>ServerHandler</code>，<code>list_tools</code> 恒返回 3 元工具（<code>crates/downstream/src/lib.rs</code>）",
        "HTTP：<code>StreamableHttpService</code> + <code>nest_service</code> 进 axum + Bearer 鉴权层",
        "<code>serve</code> 并发 stdio + HTTP（<code>tokio::select!</code> 统一关闭）",
    ],
    [
        ("02-architecture.html", "02 · 整体架构全景（下游传输能力一览）"),
        ("03-call-lifecycle.html", "03 · 一次工具调用的生命周期（客户端入口）"),
    ],
)

LESSON_13 = placeholder.build(
    "retrieval 是检索内核：定义 async 的 RetrievalStrategy 抽象与自研 BM25 默认实现；向量检索（第二部分）就建在这个抽象之上。",
    [
        "<code>RetrievalStrategy</code>(async, <code>#[async_trait]</code>) 与 <code>ScoredTool</code>（<code>crates/retrieval/src/lib.rs</code>）",
        "<code>tokenize</code> + <code>Bm25Strategy</code>（k1=1.2, b=0.75）算法",
        "<code>build_strategy</code> 工厂如何选择 bm25 / vector / hybrid",
    ],
    [
        ("04-vector-overview.html", "04 · 向量检索总览（与 BM25 对比）"),
        ("07-vector-strategy.html", "07 · VectorStrategy（内置 BM25 降级）"),
        ("08-wiring-config.html", "08 · 装配与配置（build_strategy）"),
    ],
)

LESSON_14 = placeholder.build(
    "config 是配置系统：解析 [server] / [upstream] / [retrieval] 等，并做启动期校验（fail-fast）。向量检索的 [retrieval.vector] 是其一部分。",
    [
        "<code>Config</code> / <code>RetrievalConfig</code> / <code>from_toml_str</code>（<code>crates/config/src/lib.rs</code>）",
        "<code>[server]</code>（stdio/http + API-Key）与 <code>[upstream]</code>（stdio/http transport）",
        "<code>validate()</code> 启动期校验规则",
    ],
    [
        ("08-wiring-config.html", "08 · 装配与配置（[retrieval.vector] 细节）"),
        ("02-architecture.html", "02 · 整体架构全景"),
    ],
)
```

- [ ] **Step 3: 写 part4_next.py（15 占位）**

```python
"""Part 4 (next steps) — Hybrid retrieval placeholder (awaiting M2-B)."""
import placeholder

LESSON_15 = placeholder.build(
    "Hybrid 检索用 RRF（Reciprocal Rank Fusion）融合 BM25 与向量两路排名，兼顾字面精确与语义召回。该能力属于 M2-B，尚未实现，本课待其落地后补全。",
    [
        "RRF 融合公式与为何优于单路（字面 vs 语义互补）",
        "<code>build_strategy(\"hybrid\", …)</code> 目前返回 <code>StrategyError::NotImplemented</code>，M2-B 将实现",
        "默认检索策略从 bm25 切换到 hybrid 的考量",
    ],
    [
        ("04-vector-overview.html", "04 · 向量检索总览（可插拔策略 + 透明降级）"),
        ("07-vector-strategy.html", "07 · VectorStrategy（向量一路）"),
        ("13-retrieval-bm25.html", "13 · retrieval 内核与 BM25（字面一路）"),
    ],
)
```

- [ ] **Step 4: 把 09–15 接进 registry.py**

把 Task 1 的全占位 `CONTENT` 改成：09–15 用真实占位内容，01–08 暂时仍用一行 stub（后续任务替换）。

```python
"""Single source of truth: ordered map of output filename -> lesson HTML content.

build.py imports this so the lesson set stays in sync with shell.PAGES.
"""
import shell
import part3_internals as p3
import part4_next as p4

# 01-08 (written-through lessons) are filled by later tasks; until then they use
# a one-line stub. 09-15 are navigable 'under construction' placeholder pages.
_STUB = {fname: f"<p>（待填充：{title}）</p>" for fname, title, _part in shell.PAGES}

CONTENT = {
    **_STUB,
    "09-catalog.html": p3.LESSON_09,
    "10-upstream.html": p3.LESSON_10,
    "11-gateway-metatools.html": p3.LESSON_11,
    "12-downstream.html": p3.LESSON_12,
    "13-retrieval-bm25.html": p3.LESSON_13,
    "14-config.html": p3.LESSON_14,
    "15-hybrid-rrf.html": p4.LESSON_15,
}
```

- [ ] **Step 5: 重建并校验占位页**

Run: `cd mcpgw-visual-guide/src && python build.py && cd .. && python - <<'PY'
import pathlib
h = pathlib.Path("lessons", "15-hybrid-rrf.html").read_text(encoding="utf-8")
assert "🚧 施工中" in h, "占位页缺施工中标记"
assert "RRF" in h, "占位页缺要点"
assert 'href="04-vector-overview.html"' in h, "占位页缺相关课链接"
c = pathlib.Path("lessons", "09-catalog.html").read_text(encoding="utf-8")
assert "ToolDef" in c and "🚧 施工中" in c
print("PLACEHOLDER OK")
PY`
Expected: 打印 `PLACEHOLDER OK`。

- [ ] **Step 6: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/placeholder.py mcpgw-visual-guide/src/part3_internals.py mcpgw-visual-guide/src/part4_next.py mcpgw-visual-guide/src/registry.py mcpgw-visual-guide/lessons
git commit -m "feat(guide): navigable under-construction placeholders for crates + hybrid (09-15)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: 第一部分 · 宏观全景（01–03 写透）

写透宏观三课。内容形式与样板一致：每课是 `LESSON_XX = r"""<原始 HTML>"""`，使用 shell 设计系统的卡片/流程图/表格。下面给出**每课的结构与必含事实点**（均对照 mcpgw 源码与 `docs/L1-overview.md`）；实现者据此写中文 prose，事实不得与源码冲突。

**Files:**
- Create: `mcpgw-visual-guide/src/part1_macro.py`
- Modify: `mcpgw-visual-guide/src/registry.py`

参考可读取的真实素材：`docs/L1-overview.md`（架构图与里程碑）、`docs/superpowers/specs/2026-06-08-mcpgw-progressive-discovery-design.md`（渐进式发现）、`crates/metatools/src/tools.rs`（三元工具）、`crates/downstream/src/lib.rs`（list_tools 恒 3）。

- [ ] **Step 1: 写 LESSON_01（mcpgw 是什么）**

必含结构与事实：
1. 开篇 `<p class="lead">`：mcpgw 是一个用 Rust 写的**智能 MCP 网关**，把 N 个上游 MCP server 聚合起来，但只向客户端暴露**少量元工具**。
2. `card analogy`（🔌 生活类比）：把「上百个工具一次性塞给 LLM」比作「把整个图书馆的书全堆到桌上」；mcpgw 像图书馆的**检索台**——你先问「我要做 X」，它只递给你相关的几本。
3. `<h2>` 问题：**工具爆炸**。要点：每个上游 server 暴露很多工具，聚合后工具数爆炸 → 撑爆 LLM 上下文、增加选错工具概率、破坏 prompt 缓存。用 `table.t` 列「无网关 vs mcpgw」。
4. `<h2>` 解法：**3 个元工具的渐进式发现**。用 `flow` 横向流程图：`search_tools(query)` →（拿到候选）`get_tool_details(name)` →（看清入参）`call_tool(name, args)`。三个名字必须与 `crates/metatools/src/tools.rs` 一致。
5. `card detail`（🔬 细节/代码对应）：指出客户端永远只看到 3 个工具——`crates/downstream/src/lib.rs` 的 `list_tools` **恒返回 3 个元工具**（故不声明 `list_changed` 改变可见列表）。用 `codefile` 标注路径 `crates/downstream/src/lib.rs · GatewayServer::list_tools`。
6. `card key`（✅ 关键要点）：3 句小结。
7. `card spark`（💡 设计亮点）：渐进式披露兼容所有 MCP 客户端、对 prompt 缓存友好（决策见 `docs/...progressive-discovery-design.md`）。

- [ ] **Step 2: 写 LESSON_02（整体架构全景）**

必含结构与事实：
1. lead：mcpgw 是一个 **Cargo 虚拟工作区**，按职责单一拆成多个 crate。
2. `layers`（分层架构块）列出各 crate 及一句职责，**依赖方向无环**：
   - `mcpgw`(bin, l-app)：clap CLI + `serve` 装配者。
   - `downstream`(l-main)：把 3 元工具暴露为 MCP 服务（stdio + HTTP）。
   - `gateway` + `metatools`(l-main)：ArcSwap 快照状态 + 三元工具逻辑。
   - `upstream`(l-main)：活上游 I/O（连接/摄取/路由）。
   - `retrieval` + `embedder`(l-part)：检索策略（BM25/Vector）+ 云嵌入 HTTP 后端。
   - `catalog` + `config`(l-core)：工具目录/命名空间 + 配置。
3. `card detail`：**依赖纪律**——`retrieval` 只依赖 `catalog`，**不引入 HTTP**；HTTP 依赖被隔离在独立的 `embedder` crate（reqwest 0.13）。`catalog` 不依赖任何兄弟 crate。（对照 `docs/L1-overview.md` 依赖关系段 + `crates/retrieval/Cargo.toml` / `crates/embedder/Cargo.toml`。）
4. **传输能力一览** `table.t`（上游/下游 × stdio/HTTP），与 `docs/L1-overview.md` 同名表一致。
5. `card key` + `card spark`（亮点：把「会爆炸的工具列表」收敛成「恒定 3 元工具」，检索逻辑全在网关内部，可独立演进）。
6. 末尾一句导航到第二部分：检索策略是网关的核心可插拔件，下一部分深入向量检索。

- [ ] **Step 3: 写 LESSON_03（一次工具调用的生命周期）**

必含结构与事实：
1. lead：跟随一次「客户端想调用某上游工具」的完整数据流。
2. `vflow`（纵向步骤流）：
   1. 客户端连上 `downstream`（stdio 或 HTTP），`list_tools` 看到 3 元工具。
   2. 客户端调 `search_tools("…")` → `metatools` 在不可变 `GatewaySnapshot` 上跑检索策略（BM25 或 Vector）→ 返回候选 `ToolSummary` 列表（已按相关性排序）。
   3. 客户端调 `get_tool_details(qualified_name)` → 从 catalog 取该工具完整 `ToolDef`（含 input schema）。
   4. 客户端调 `call_tool(qualified_name, args)` → `metatools` 经 catalog 查 `(server, tool)`（**绝不拆 `__`**）→ 路由到 `upstream` 对应 handle 转发，带每调用超时。
3. `card detail`：快照与重建——读路径无锁（`ArcSwap`），上游 `tools/list_changed` 触发后台 `rebuild_snapshot`（build-then-swap）而不阻塞检索（`crates/gateway/src/lib.rs`）。用 `codefile` 标注 `crates/metatools/src/tools.rs · search_tools/get_tool_details/call_tool`。
4. `card warn`：日志走 **stderr**，stdout 留给 MCP 协议帧。
5. `card key` 小结三步心智：检索 → 详情 → 执行。
6. `card spark`：这正是「渐进式披露」在数据流上的体现——LLM 永远只面对 3 个稳定入口。

- [ ] **Step 4: 接进 registry.py**

把 `registry.py` 顶部加 `import part1_macro as p1`，并在 `CONTENT` 字典里覆盖 01–03：

```python
import part1_macro as p1
```
在 `CONTENT = { **_STUB, …}` 中加入：
```python
    "01-what-is-mcpgw.html": p1.LESSON_01,
    "02-architecture.html": p1.LESSON_02,
    "03-call-lifecycle.html": p1.LESSON_03,
```

- [ ] **Step 5: 重建并校验**

Run: `cd mcpgw-visual-guide/src && python build.py && cd .. && python - <<'PY'
import pathlib
for f, must in [
    ("01-what-is-mcpgw.html", ["search_tools","get_tool_details","call_tool","class=\"card analogy\""]),
    ("02-architecture.html", ["embedder","retrieval","class=\"layers\"","传输能力"]),
    ("03-call-lifecycle.html", ["GatewaySnapshot","rebuild","class=\"vflow\""]),
]:
    h = pathlib.Path("lessons", f).read_text(encoding="utf-8")
    for m in must:
        assert m in h, f"{f} 缺 {m!r}"
    assert "（待填充" not in h, f"{f} 仍是 stub"
print("MACRO OK")
PY`
Expected: 打印 `MACRO OK`。

- [ ] **Step 6: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/part1_macro.py mcpgw-visual-guide/src/registry.py mcpgw-visual-guide/index.html mcpgw-visual-guide/lessons
git commit -m "feat(guide): part 1 macro overview lessons 01-03 (what/architecture/lifecycle)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: 向量检索专章（04 总览 + 05 Embedder）

写透向量章前两课。事实点全部对照真实源码：`crates/retrieval/src/{lib.rs,vector.rs,embedder.rs}`、`crates/embedder/src/lib.rs`、`docs/L2-components/{retrieval.md,embedder.md}`。

**Files:**
- Create: `mcpgw-visual-guide/src/part2_vector.py`（本任务先放 `LESSON_04`、`LESSON_05`，06–08 由 Task 5/6 追加到同文件）
- Modify: `mcpgw-visual-guide/src/registry.py`

- [ ] **Step 1: 写 LESSON_04（向量检索总览）**

必含结构与事实：
1. lead：BM25 是**字面**匹配（共享词才命中）；向量检索是**语义**匹配（意思相近即命中）。mcpgw 把检索做成**可插拔策略**，向量策略**内置 BM25 作透明降级**。
2. `card analogy`（🔌）：BM25 像「按书名里的关键词找书」；向量像「告诉图书管理员你想干什么，他凭理解推荐」。
3. `cols`（两栏对比）BM25 vs 向量：命中机制、对同义/改写的鲁棒性、是否需外部服务、离线可用性。
4. `card detail`（🔬）：关键设计——**透明降级**。`VectorStrategy` 持有 `embedder` + 内置 `Bm25Strategy` + `degraded` 标志；`index` **总是先建 BM25**，再尝试嵌入整个目录，失败则 `degraded=true`；`search` 在 degraded / 无向量 / 单次查询嵌入失败时回落 BM25。用 `codefile` 标注 `crates/retrieval/src/vector.rs · VectorStrategy::{index,search}`，并给一段精简的真实代码（degraded 判断那几行）。
5. 一个**语义增益**示例（与门控冒烟一致）：查询 `"communicate with my team"` 与任何工具描述无共享词 → BM25 召回为空，向量却能把 `slack__post_message` 排第一。用 `flow` 或小表格表达。
6. `card key`：可插拔 + 默认仍 bm25 + 向量永不"硬失败"（坏了就退回 BM25）。
7. `card spark`：降级是**透明**的——调用方拿到的永远是「尽力而为的最佳排序」，不需要处理 embedder 故障。

- [ ] **Step 2: 写 LESSON_05（Embedder 抽象 & OpenAiEmbedder）**

必含结构与事实：
1. lead：要做向量检索，先要把文本变成向量。mcpgw 用一个**与厂商无关**的 `Embedder` trait 抽象这件事，真实 HTTP 实现放在独立的 `embedder` crate，于是 `retrieval` **不引入任何 HTTP 依赖**。
2. `codefile` 展示 trait（来自 `crates/retrieval/src/embedder.rs`）：
   - `async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>`（一批文本各转一个向量、**顺序对应**、**all-or-nothing**）。
   - `fn dim(&self) -> usize`。
   - `EmbedError::{Provider(String), Dimension{expected,got}}`（**provider 无关**）。
3. `card detail`（🔬）OpenAiEmbedder（`crates/embedder/src/lib.rs`）：
   - `POST {base_url}/embeddings`，`bearer_auth(api_key)`，body `{model, input}`。
   - 按响应 `index` **排序**，校验数量与 index 连续性、可选 `dim` 校验。
   - 非 2xx 时把**响应体截断片段**（最多 500 字符）放进错误（便于排错），**绝不回显 Authorization**。
   - 空输入短路返回 `Ok(vec![])`。
   - 是 workspace 里**唯一**依赖 reqwest 的 crate（0.13，rustls）。
4. `card warn`（⚠️）密钥：API key 来自**环境变量**，构造 `OpenAiEmbedder` 时传入的是 token 值；但配置里存的是**env 变量名**（见第 08 课），错误信息只提变量名不提值。
5. `card detail` 测试替身：`MockEmbedder`（`testkit` feature）把 token 哈希分桶生成确定性伪向量，共享 token 的文本余弦更高，且暴露 `calls`/`seen` 供缓存断言（第 06 课用到）。
6. `card key` + `card spark`（亮点：trait 边界把「HTTP/厂商」与「检索逻辑」彻底解耦——retrieval 可在无网络下编译/测试）。

- [ ] **Step 3: 接进 registry.py**

`registry.py` 顶部加 `import part2_vector as p2`，并在 `CONTENT` 覆盖：

```python
    "04-vector-overview.html": p2.LESSON_04,
    "05-embedder.html": p2.LESSON_05,
```

- [ ] **Step 4: 重建并校验**

Run: `cd mcpgw-visual-guide/src && python build.py && cd .. && python - <<'PY'
import pathlib
o = pathlib.Path("lessons", "04-vector-overview.html").read_text(encoding="utf-8")
for m in ["VectorStrategy","degraded","BM25","communicate with my team"]:
    assert m in o, f"04 缺 {m!r}"
e = pathlib.Path("lessons", "05-embedder.html").read_text(encoding="utf-8")
for m in ["Embedder","EmbedError","OpenAiEmbedder","reqwest","bearer"]:
    assert m in e, f"05 缺 {m!r}"
assert "（待填充" not in o and "（待填充" not in e
print("VEC1 OK")
PY`
Expected: 打印 `VEC1 OK`。

- [ ] **Step 5: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/part2_vector.py mcpgw-visual-guide/src/registry.py mcpgw-visual-guide/index.html mcpgw-visual-guide/lessons
git commit -m "feat(guide): vector chapter 04-05 (overview + Embedder/OpenAiEmbedder)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: 向量检索专章（06 CachingEmbedder + 07 VectorStrategy）

写透缓存与核心策略两课。对照 `crates/retrieval/src/{caching.rs,vector.rs}`。

**Files:**
- Modify: `mcpgw-visual-guide/src/part2_vector.py`（追加 `LESSON_06`、`LESSON_07`）
- Modify: `mcpgw-visual-guide/src/registry.py`

- [ ] **Step 1: 写 LESSON_06（CachingEmbedder 缓存）**

必含结构与事实（源：`crates/retrieval/src/caching.rs`）：
1. lead：每次快照重建都要给整目录算嵌入，但工具大多没变；`CachingEmbedder` 是个**装饰器**，按**文本内容哈希**记忆向量，只把 cache-miss 的文本转发给内层 embedder。
2. `card analogy`（🔌）：像背单词卡片——见过的词直接翻答案，只有没见过的才查词典。
3. `card detail`（🔬）内容哈希：用 **FNV-1a**（`hash_text`，初值 `0xcbf29ce484222325`，乘 `0x100000001b3`）对文本求 64-bit 哈希作 key；命中即复用、未命中才嵌入。给 `codefile` 标注 `crates/retrieval/src/caching.rs · CachingEmbedder::embed` + 精简代码（三段：收集 miss / 只嵌 miss / 按原序重组）。
4. `card warn`（⚠️）**锁纪律**：实现用 `std::sync::Mutex`，**绝不跨 `.await` 持锁**——三个不相交的临界区（读 miss、写入、重组），中间的 `inner.embed(...).await` 不持锁。这是避免阻塞异步执行器的关键。
5. `card detail` **insert-only 不变量**：重组步骤的 `.expect("just inserted/hit")` 之所以**永不 panic**，是因为缓存只插入、从不淘汰，所以每个 key 此刻必然存在；将来若加 LRU/TTL 淘汰，必须在此处重新处理 miss（代码注释已写明）。
6. `card detail` **跨 rebuild 持久**：在装配层 `CachingEmbedder` 只构造**一次**（见第 08 课），故缓存跨多次 `rebuild_snapshot` 存活——未变工具不会被重复嵌入（有门控测试断言内层 `MockEmbedder.calls` 第二次重建不增长）。
7. `card key` + `card spark`（亮点：装饰器模式把「省钱省延迟」正交地叠加在任意 `Embedder` 上，VectorStrategy 完全无感）。

- [ ] **Step 2: 写 LESSON_07（VectorStrategy 余弦 + 降级）**

必含结构与事实（源：`crates/retrieval/src/vector.rs`）：
1. lead：`VectorStrategy` 在云嵌入上做**暴力余弦检索**（目录小，线性扫描足够），并在嵌入不可用时**透明回落** BM25。
2. `card detail`（🔬）数据结构：持有 `embedder: Arc<dyn Embedder>`、内置 `bm25: Bm25Strategy`、`vectors: Vec<(qualified_name, description, 归一化向量)>`、`degraded: bool`。每个工具被嵌入的文本是 `tool_text = "{qualified_name}\n{description}"`。
3. `vflow` 描述 `index` 流程：① 总是先 `bm25 = Bm25Strategy::new(); bm25.index()`；② 收集 `tool_text`；③ `embedder.embed(&texts).await`：
   - `Ok(vecs)` 且 **数量匹配** → 归一化后存入 `vectors`，`degraded=false`；
   - `Ok(vecs)` 但**数量不匹配** → 这违反 all-or-nothing/顺序契约，**降级 BM25**（clear vectors、degraded=true）而非建错位索引；
   - `Err` → 降级 BM25。
4. `vflow`/文本描述 `search` 流程：若 `degraded` 或 `vectors` 空 → 直接 `bm25.search`；否则嵌入 query（单次失败也回落 BM25）→ 对每个工具算 `dot(归一化 query, 归一化向量)` 即余弦 → 按 `score 降序 + qualified_name 升序` 稳定排序 → 截断 `top_k`。
5. `card detail` **零范数防 NaN**：`normalize` 对零向量原样返回（不除零），故余弦不会出 NaN。给 `codefile` 标注 `crates/retrieval/src/vector.rs · normalize` + 那几行。
6. `card warn`（⚠️）**双重降级**两条路径要讲清：index-time（整批嵌入失败/数量不符）与 query-time（单次查询嵌入失败）都回落 BM25。
7. `card key`（余弦=归一化点积、稳定排序、永不 NaN、永不硬失败）+ `card spark`（亮点：把「语义检索」与「字面兜底」缝进同一个策略，对上层就是一个普通 `RetrievalStrategy`）。

- [ ] **Step 3: 接进 registry.py**

`CONTENT` 覆盖：

```python
    "06-caching-embedder.html": p2.LESSON_06,
    "07-vector-strategy.html": p2.LESSON_07,
```

- [ ] **Step 4: 重建并校验**

Run: `cd mcpgw-visual-guide/src && python build.py && cd .. && python - <<'PY'
import pathlib
c = pathlib.Path("lessons", "06-caching-embedder.html").read_text(encoding="utf-8")
for m in ["CachingEmbedder","FNV","await","insert-only"]:
    assert m in c, f"06 缺 {m!r}"
v = pathlib.Path("lessons", "07-vector-strategy.html").read_text(encoding="utf-8")
for m in ["normalize","degraded","top_k","qualified_name"]:
    assert m in v, f"07 缺 {m!r}"
assert "（待填充" not in c and "（待填充" not in v
print("VEC2 OK")
PY`
Expected: 打印 `VEC2 OK`。

- [ ] **Step 5: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/part2_vector.py mcpgw-visual-guide/src/registry.py mcpgw-visual-guide/index.html mcpgw-visual-guide/lessons
git commit -m "feat(guide): vector chapter 06-07 (CachingEmbedder + VectorStrategy)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6: 向量检索专章（08 装配与配置）

写透向量章收口课：策略/embedder 如何被构造与注入、配置长什么样、启动期如何 fail-fast。对照 `crates/retrieval/src/lib.rs`（`build_strategy`）、`crates/mcpgw/src/main.rs`（`build_embedder`/`prepare_state`）、`crates/config/src/lib.rs`（`VectorConfig`）、`docs/L2-components/config.md`。

**Files:**
- Modify: `mcpgw-visual-guide/src/part2_vector.py`（追加 `LESSON_08`）
- Modify: `mcpgw-visual-guide/src/registry.py`

- [ ] **Step 1: 写 LESSON_08（装配与配置）**

必含结构与事实：
1. lead：把前几课的零件接起来——配置选 `strategy="vector"` 时，启动期如何构造 embedder、注入 gateway、并在缺凭证时**立刻失败**。
2. `card detail`（🔬）`build_strategy`（`crates/retrieval/src/lib.rs`）：`build_strategy(name, embedder: Option<&Arc<dyn Embedder>>)`：`"bm25"` 无需 embedder；`"vector"` **需要** embedder，否则 `StrategyError::EmbedderRequired`；`"hybrid"`/未知 → `NotImplemented`（M2-B）。给 `codefile` + 精简 match 代码。**默认策略仍是 bm25**。
3. `card detail` `build_embedder`（`crates/mcpgw/src/main.rs`）：当 `strategy=="vector"` 时读 `[retrieval.vector]`，从 `api_key_env` 命名的环境变量取 key（**fail-fast**，错误只提变量名）→ 构造 `OpenAiEmbedder` → **包一层 `CachingEmbedder`（只构造一次，故缓存跨 rebuild 持久）** → `Some(Arc<dyn Embedder>)`；否则 `None`。`prepare_state` 据此分支 `GatewayState::with_embedder` vs `::new`。给 `codefile` + 精简代码。
4. `card detail` 配置 `VectorConfig`（`crates/config/src/lib.rs`，`deny_unknown_fields`）：用 `table.t` 列字段：`base_url`(默认 OpenAI)、`model`、`api_key_env`、`dim?`、`timeout_ms?`、`batch_size?`。给一段 `pre.code` 的 TOML 示例：
   ```toml
   [retrieval]
   strategy = "vector"

   [retrieval.vector]
   base_url = "https://api.openai.com/v1"
   model = "text-embedding-3-small"
   api_key_env = "OPENAI_API_KEY"
   ```
5. `card warn`（⚠️）三个易错点：
   - `batch_size` 当前是**预留/未启用**字段（OpenAiEmbedder 一次性发全部输入，不分块）——不要以为它已生效。
   - `strategy="vector"` **只在 `serve`（活网关）下生效**；离线的 `search`/`get-details` CLI 不注入 embedder。
   - `validate()` 在 `strategy=="vector"` 时**要求**存在 `[retrieval.vector]` 段。
6. `card key`（fail-fast、缓存只建一次、默认 bm25、密钥只存 env 名）+ `card spark`（亮点：配置层把"要不要向量、向量打哪、用哪个 key"声明化，装配层把可靠性约束在启动期一次性兑现）。
7. 末尾一句：向量章到此结束；下一步（Hybrid/RRF）见第四部分占位（待 M2-B）。

- [ ] **Step 2: 接进 registry.py**

`CONTENT` 覆盖：

```python
    "08-wiring-config.html": p2.LESSON_08,
```

- [ ] **Step 3: 重建并校验**

Run: `cd mcpgw-visual-guide/src && python build.py && cd .. && python - <<'PY'
import pathlib
w = pathlib.Path("lessons", "08-wiring-config.html").read_text(encoding="utf-8")
for m in ["build_strategy","build_embedder","EmbedderRequired","api_key_env","batch_size","[retrieval.vector]"]:
    assert m in w, f"08 缺 {m!r}"
assert "（待填充" not in w
print("VEC3 OK")
PY`
Expected: 打印 `VEC3 OK`。

- [ ] **Step 4: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/part2_vector.py mcpgw-visual-guide/src/registry.py mcpgw-visual-guide/index.html mcpgw-visual-guide/lessons
git commit -m "feat(guide): vector chapter 08 (wiring + [retrieval.vector] config)

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 7: 死链检查 + README + 全站验证

补一个 stdlib-only 内部死链检查脚本、写 README、做整站验证收口（spec 验收 1–5）。

**Files:**
- Create: `mcpgw-visual-guide/src/check_links.py`
- Create: `mcpgw-visual-guide/README.md`

- [ ] **Step 1: 写 check_links.py（内部相对链接死链检查）**

仅用标准库；解析 `index.html` + `lessons/*.html` 里的 `href="..."`，忽略外链（`http`/`mailto`/`#`/`data:`），把相对链接解析到文件系统验证存在。

```python
"""Check internal relative links in the built site resolve to real files.

stdlib-only. Scans index.html + lessons/*.html for href="…"; ignores external
(http/https/mailto), in-page (#…) and data: links; resolves the rest against
the file's directory and asserts the target exists.

Usage:
    cd mcpgw-visual-guide/src && python check_links.py
Exit code 0 = all good, 1 = dead links found.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
HREF = re.compile(r'href="([^"]+)"')


def page_files():
    yield os.path.join(ROOT, "index.html")
    lessons = os.path.join(ROOT, "lessons")
    for name in sorted(os.listdir(lessons)):
        if name.endswith(".html"):
            yield os.path.join(lessons, name)


def is_external(href):
    return (
        href.startswith(("http://", "https://", "mailto:", "data:", "#"))
        or href.strip() == ""
    )


def main():
    dead = []
    for path in page_files():
        base = os.path.dirname(path)
        html = open(path, encoding="utf-8").read()
        for href in HREF.findall(html):
            if is_external(href):
                continue
            target = href.split("#", 1)[0].split("?", 1)[0]
            if not target:
                continue
            resolved = os.path.normpath(os.path.join(base, target))
            if not os.path.exists(resolved):
                dead.append((os.path.relpath(path, ROOT), href))
    if dead:
        print("DEAD LINKS:")
        for src, href in dead:
            print(f"  {src} -> {href}")
        sys.exit(1)
    print("All internal links OK")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: 跑死链检查**

Run: `cd mcpgw-visual-guide/src && python check_links.py`
Expected: 打印 `All internal links OK`，退出码 0。

- [ ] **Step 3: 写 README.md**

`mcpgw-visual-guide/README.md`（要点）：
- 一句话简介：mcpgw 的图解教程（中文、自包含 HTML、无依赖）。
- 如何阅读：直接浏览器打开 `index.html`（`file://` 即可），或 `python -m http.server 8000`。
- 教程结构：4 部分 15 课的清单（标「✍️ 写透」/「🚧 施工中」），第一部分 1–3 与第二部分 4–8 已写透，9–15 占位。
- 如何重新生成：`cd src && python build.py`；死链检查 `python check_links.py`。
- 项目结构：列 `src/`（shell/partN/placeholder/registry/build/check_links）、`index.html`、`lessons/`。
- 后续计划：补全各 crate 内部（9–14）、M2-B 后写 Hybrid（15）、加 PDF（`build_print.py`）与 GitHub Pages/CI（参考 `../langchain-visual-guide`）。
- 一句说明：内容对照 mcpgw `master`（M2-A 合并后）真实源码，源码引用以「文件 + 符号名」为主（不写死行号）。

- [ ] **Step 4: 全站验证（重建 + 死链 + 完整性）**

Run: `cd mcpgw-visual-guide/src && python build.py && python check_links.py && cd .. && python - <<'PY'
import pathlib
# 16 files exist
assert pathlib.Path("index.html").exists()
lessons = sorted(p.name for p in pathlib.Path("lessons").glob("*.html"))
assert len(lessons) == 15, f"应有 15 课，实际 {len(lessons)}"
# written-through lessons (01-08) carry no stub marker
for n in ["01-what-is-mcpgw","02-architecture","03-call-lifecycle",
          "04-vector-overview","05-embedder","06-caching-embedder",
          "07-vector-strategy","08-wiring-config"]:
    h = pathlib.Path("lessons", n+".html").read_text(encoding="utf-8")
    assert "（待填充" not in h, f"{n} 仍是 stub"
# placeholders (09-15) carry the under-construction marker
for n in ["09-catalog","10-upstream","11-gateway-metatools","12-downstream",
          "13-retrieval-bm25","14-config","15-hybrid-rrf"]:
    h = pathlib.Path("lessons", n+".html").read_text(encoding="utf-8")
    assert "🚧 施工中" in h, f"{n} 不是占位页"
# index TOC lists 15 lessons
idx = pathlib.Path("index.html").read_text(encoding="utf-8")
assert idx.count('class="n"') == 15
print("SITE OK — 1 index + 15 lessons, 8 written / 7 placeholders")
PY`
Expected: 打印 `SITE OK — 1 index + 15 lessons, 8 written / 7 placeholders`。

- [ ] **Step 5: 提交**

```bash
cd /home/verden/course/mcpgw
git add mcpgw-visual-guide/src/check_links.py mcpgw-visual-guide/README.md
git commit -m "feat(guide): internal link checker + README + site verification

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## 收尾（全部任务完成后）

1. 派发最终整体 review：检查站点可 `file://` 打开、导航/进度条/深色模式工作、内部无死链、写透 8 课事实与 mcpgw 源码一致、占位 7 课可导航且信息有用。
2. 处理 blocking 项（如有）。
3. 用 superpowers:finishing-a-development-branch 收口（本次直接在 `master` 上按任务提交，或按需开 `feat/visual-guide` 分支——执行前与用户确认分支策略）。

## 后续（不在首版范围）
- 写透第三部分各 crate 内部（09–14）。
- M2-B 完成后写透 Hybrid/RRF（15）。
- 加 `build_print.py` → PDF；加 GitHub Pages + CI（deploy / 防漂移 / 死链），参考 `../langchain-visual-guide/.github/workflows`。
- 可选 quizzes、glossary、首页搜索副本（搜索 JS 已随 shell 移植）。
