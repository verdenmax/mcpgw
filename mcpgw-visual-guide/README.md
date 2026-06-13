# mcpgw 图解教程

mcpgw 的图解教程：**中文、自包含 HTML、零依赖**——直接用浏览器打开就能读，配真实代码对应、折叠深挖与设计亮点。

> 🌐 **在线阅读**：<https://verdenmax.github.io/mcpgw/>（推送到 `master` 时由 GitHub Actions 自动构建并部署）

## 如何阅读

- **最简单**：双击 / 用浏览器打开 `index.html`（`file://` 协议即可，无需任何服务器）。
- **本地服务器**（可选，体验更接近线上）：

  ```bash
  cd mcpgw-visual-guide
  python -m http.server 8000
  # 浏览器访问 http://localhost:8000
  ```

## 教程结构（4 部分 · 15 课）

> ✍️ = 写透　🚧 = 施工中（可导航的占位页）
> 第一部分 1–3 与第二部分 4–8 已写透，9–15 为占位。

**第一部分 · 宏观全景**
1. ✍️ what is mcpgw —— mcpgw 是什么
2. ✍️ architecture —— 整体架构全景
3. ✍️ call lifecycle —— 一次工具调用的生命周期

**第二部分 · 向量检索专章**
4. ✍️ vector overview —— 向量检索全景
5. ✍️ embedder —— OpenAI Embedder
6. ✍️ caching embedder —— 带缓存的 Embedder
7. ✍️ vector strategy —— 向量检索策略
8. ✍️ wiring & config —— 装配与配置

**第三部分 · 各 crate 内部**
9. 🚧 catalog —— 工具目录与命名空间
10. 🚧 upstream —— 上游连接与聚合
11. 🚧 gateway metatools —— 三个元工具
12. 🚧 downstream —— 对外暴露 MCP 服务
13. 🚧 retrieval / BM25 —— 检索与 BM25
14. 🚧 config —— 配置系统
15. 🚧 hybrid / RRF —— 混合检索

**第四部分 · 后续**
（15 课归入此处的 Hybrid 主题，M2-B 落地后写透）

## 如何重新生成

教程由一个**纯标准库**的静态站点生成器产出：

```bash
cd src
python build.py        # 生成 index.html + lessons/01..15-*.html
python check_links.py  # 内部相对链接死链检查（退出码 0 = 全部 OK）
```

## 项目结构

```
mcpgw-visual-guide/
├── README.md
├── index.html                 # 生成物：目录页（带搜索）
├── lessons/                    # 生成物：01..15-*.html
└── src/                        # 生成器（标准库，无第三方依赖）
    ├── shell.py                # 页面外壳：HTML/CSS/JS、PAGES 清单、导航与目录页
    ├── part1_macro.py          # 第一部分 1–3 课内容
    ├── part2_vector.py         # 第二部分 4–8 课内容
    ├── part3_internals.py      # 第三部分 9–14 课内容
    ├── part4_next.py           # 第四部分 / 第 15 课内容
    ├── placeholder.py          # 「🚧 施工中」占位页构造
    ├── registry.py             # 文件名 → 内容映射（含 key 合法性断言）
    ├── build.py                # 入口：渲染全部页面到上级目录
    └── check_links.py          # 内部死链检查（标准库）
```

## 后续计划

- 补全第三部分各 crate 内部（9–14）。
- M2-B 落地后写透 Hybrid / RRF（15）。
- 加 PDF 导出（`build_print.py`）。
- ~~加 GitHub Pages / CI 自动发布~~ ✅ 已配 `.github/workflows/pages.yml`（推送即构建 + 校验无漂移 + 死链检查 + 部署 Pages）。

## 内容来源

内容对照 mcpgw `master`（M2-A 合并后）的真实源码。源码引用以**「文件 + 符号名」**为主（如 `crates/retrieval/src/lib.rs · build_strategy`），不写死行号，便于随源码演进保持准确。
