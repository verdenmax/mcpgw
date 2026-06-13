"""Part 1 (macro overview): lessons 01-03 — what / architecture / call lifecycle.

These three lessons are written through (real teaching content). Every factual
claim is anchored to the mcpgw source tree and docs/L1-overview.md:

  - 3 meta-tools: crates/metatools/src/tools.rs (search_tools / get_tool_details / call_tool)
  - list_tools always returns the 3 meta-tools: crates/downstream/src/lib.rs
  - dependency discipline (retrieval has NO http; embedder isolates reqwest 0.13):
      crates/retrieval/Cargo.toml · crates/embedder/Cargo.toml
  - ArcSwap snapshot + rebuild_snapshot (build-then-swap): crates/gateway/src/lib.rs
"""

# ---------------------------------------------------------------------------
LESSON_01 = r"""
<p class="lead">
mcpgw 是一个用 <strong>Rust</strong> 写的<strong>智能 MCP（Model Context Protocol）网关</strong>。
它把 N 个上游 MCP server 聚合起来，但对客户端<strong>只暴露少量「元工具」</strong>——
由网关在内部做工具检索与按需加载，避免「把上百个工具一次性塞给 LLM」导致的上下文爆炸与选错工具。
</p>

<div class="card analogy">
  <div class="tag">🔌 生活类比</div>
  把「上百个工具一次性塞给 LLM」想成<strong>把整座图书馆的书全堆到桌上</strong>——桌子塞满了，你也根本找不到要用的那本。
  mcpgw 更像图书馆门口的<strong>检索台</strong>：你先说一句「我想做 X」，它只把<strong>相关的几本</strong>递给你；
  需要细看时再帮你取来完整那一本；想借走时它替你去书库里把书拿出来。
  LLM 始终只面对检索台这一个稳定窗口，而不是整面书墙。
</div>

<h2>它解决什么问题：工具爆炸</h2>
<p>每个上游 MCP server 都会暴露一堆工具。把很多 server 聚合到一起，工具总数会迅速膨胀。
直接把这张长长的工具清单丢给模型，会同时引发三个麻烦：</p>

<table class="t">
  <tr><th>维度</th><th>没有网关（直接聚合全部工具）</th><th>mcpgw（渐进式发现）</th></tr>
  <tr><td><strong>上下文占用</strong></td><td>上百个工具的 schema 全塞进 prompt，撑爆上下文窗口</td><td>客户端始终只看到 <strong>3 个元工具</strong>，占用恒定</td></tr>
  <tr><td><strong>选择准确率</strong></td><td>候选越多，模型越容易选错工具</td><td>先按查询检索，只把<strong>相关候选</strong>交给模型</td></tr>
  <tr><td><strong>prompt 缓存</strong></td><td>上游增删工具 → 工具数组变化 → 缓存失效</td><td>工具数组<strong>永不变化</strong> → 缓存稳定命中</td></tr>
</table>

<h2>它的解法：3 个元工具的渐进式发现</h2>
<p>mcpgw 不把真实工具直接暴露出去，而是只暴露三个固定的「元工具」，让客户端按
<strong>检索 → 看详情 → 执行</strong>的节奏，分步把需要的工具「问」出来：</p>

<div class="flow">
  <div class="node hl"><div class="nt">search_tools(query)</div><div class="nd">用自然语言查询，拿到相关工具候选</div></div>
  <div class="arrow">→</div>
  <div class="node"><div class="nt">get_tool_details(name)</div><div class="nd">看清某个工具的完整入参 schema</div></div>
  <div class="arrow">→</div>
  <div class="node"><div class="nt">call_tool(name, args)</div><div class="nd">带参数真正执行该上游工具</div></div>
</div>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  无论上游有多少工具、是否增减，客户端<strong>永远只看到这 3 个元工具</strong>。
  下游 MCP 服务的 <span class="inline">list_tools</span> 恒定返回这 3 个，因此<strong>不需要</strong>用
  <span class="inline">list_changed</span> 去改变模型可见的工具列表（上游变化只在网关内部触发重建检索索引）。
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/downstream/src/lib.rs · GatewayServer::list_tools</span></div>
<pre><span class="cm">// 客户端能看到的工具集合恒定为 3 个元工具</span>
<span class="kw">async fn</span> <span class="fn">list_tools</span>(&amp;self, _req, _ctx)
    -&gt; Result&lt;ListToolsResult, McpError&gt; {
    Ok(ListToolsResult::with_all_items(<span class="fn">meta_tools</span>()))
}
<span class="cm">// meta_tools() == [search_tools, get_tool_details, call_tool]</span></pre>
  </div>
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li>mcpgw = 聚合 N 个上游 MCP server 的<strong>智能网关</strong>，对外只露 3 个稳定元工具。</li>
    <li>核心能力是<strong>渐进式工具发现</strong>：检索 → 详情 → 执行，把工具按需「问」出来。</li>
    <li>三个元工具的真实名字是 <span class="inline">search_tools</span> / <span class="inline">get_tool_details</span> / <span class="inline">call_tool</span>。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  渐进式披露做在<strong>网关（server）侧</strong>，因此<strong>兼容所有 MCP 客户端、零改造</strong>；
  又因为对外的工具数组永不变化，对 <strong>prompt 缓存友好</strong>。
  这一选择背后还有现实考量：<span class="inline">tools/list_changed</span> 在协议里是可选能力，主流客户端运行中刷新并不可靠，
  所以 mcpgw 不依赖它来改变可见工具列表（决策详见
  <span class="inline">docs/superpowers/specs/2026-06-08-mcpgw-progressive-discovery-design.md</span>）。
</div>
"""

# ---------------------------------------------------------------------------
LESSON_02 = r"""
<p class="lead">
mcpgw 是一个 <strong>Cargo 虚拟工作区（virtual workspace）</strong>，按「职责单一」拆成多个 crate。
每个 crate 只做一件事，彼此<strong>依赖方向无环</strong>，于是检索逻辑、上游 I/O、对外服务都能独立演进。
</p>

<h2>分层全景：各 crate 与它的一句话职责</h2>
<p>从对外的可执行程序，到最底层的纯数据结构，自上而下大致是这样几层：</p>

<div class="layers">
  <div class="layer l-app">
    <div class="lh"><span class="badge">bin</span><span class="name">mcpgw</span></div>
    <div class="ld">唯一的集成者：clap CLI + <span class="inline">serve</span> 装配者，把上游、网关、检索、配置拼起来。</div>
  </div>
  <div class="layer l-main">
    <div class="lh"><span class="badge">下游</span><span class="name">downstream</span></div>
    <div class="ld">把 3 个元工具暴露为真正的 MCP 服务（stdio + Streamable HTTP）。</div>
  </div>
  <div class="layer l-main">
    <div class="lh"><span class="badge">网关</span><span class="name">gateway + metatools</span></div>
    <div class="ld"><span class="inline">ArcSwap</span> 快照状态 + 三个元工具逻辑（在不可变快照上检索/路由）。</div>
  </div>
  <div class="layer l-main">
    <div class="lh"><span class="badge">上游</span><span class="name">upstream</span></div>
    <div class="ld">活的上游 MCP I/O：连接、工具摄取、<span class="inline">call_tool</span> 路由转发。</div>
  </div>
  <div class="layer l-part">
    <div class="lh"><span class="badge">检索</span><span class="name">retrieval + embedder</span></div>
    <div class="ld">检索策略（BM25 / Vector）+ 云端嵌入的 HTTP 后端（检索栈里只有 embedder 直连 HTTP）。</div>
  </div>
  <div class="layer l-core">
    <div class="lh"><span class="badge">内核</span><span class="name">catalog + config</span></div>
    <div class="ld">工具目录与 <span class="inline">{server}__{name}</span> 命名空间 + 配置解析/校验。</div>
  </div>
</div>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应：依赖纪律</div>
  <p>分层之所以稳，靠的是几条刻意定下的依赖规则（对照 <span class="inline">docs/L1-overview.md</span> 依赖关系段）：</p>
  <ul>
    <li><strong><span class="inline">catalog</span> 不依赖任何兄弟 crate</strong>，只用 serde——它是最底层的纯数据结构。</li>
    <li><strong><span class="inline">retrieval</span> 只依赖 <span class="inline">catalog</span></strong>，<strong>不引入任何 HTTP 依赖</strong>；
      <span class="inline">build_strategy</span> 故意接受策略名字符串（+ 可选 <span class="inline">Embedder</span>）而非配置类型，保持检索内核可独立复用。</li>
    <li><strong>检索栈的 HTTP 客户端被隔离在独立的 <span class="inline">embedder</span> crate</strong>（<span class="inline">reqwest 0.13</span> + rustls）——
      它是工作区里<strong>唯一直接以 reqwest 作 HTTP 客户端</strong>的 crate，承载真实嵌入后端 <span class="inline">OpenAiEmbedder</span>；
      上游/下游方向的 HTTP 则走 rmcp 的 Streamable HTTP 传输与 axum（见下方传输表）。</li>
  </ul>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/Cargo.toml · crates/embedder/Cargo.toml</span></div>
<pre><span class="cm"># crates/retrieval/Cargo.toml —— 没有 reqwest / http</span>
[dependencies]
catalog = { path = <span class="st">"../catalog"</span> }
async-trait = { workspace = <span class="kw">true</span> }

<span class="cm"># crates/embedder/Cargo.toml —— HTTP 被隔离在这里</span>
[dependencies]
retrieval = { path = <span class="st">"../retrieval"</span> }
reqwest = { version = <span class="st">"0.13"</span>, features = [<span class="st">"json"</span>, <span class="st">"rustls"</span>] }</pre>
  </div>
</div>

<h2>传输能力一览</h2>
<p>网关在「上游」和「下游」两个方向上都支持 stdio 与 HTTP（与 <span class="inline">docs/L1-overview.md</span> 同名表一致）：</p>

<table class="t">
  <tr><th>方向</th><th>stdio</th><th>HTTP（Streamable HTTP）</th></tr>
  <tr>
    <td><strong>上游</strong>（连接被聚合的 MCP server）</td>
    <td>✅ 子进程（<span class="inline">command</span>/<span class="inline">args</span> + env allow-list）</td>
    <td>✅ 远程 <span class="inline">url</span> + 静态鉴权（<span class="inline">bearer_env</span> 原始 token、<span class="inline">headers</span> 头名→env）</td>
  </tr>
  <tr>
    <td><strong>下游</strong>（向客户端暴露 3 个元工具）</td>
    <td>✅ <span class="inline">serve</span> over stdio</td>
    <td>✅ 默认 <span class="inline">127.0.0.1:8970</span> <span class="inline">/mcp</span> + 多 key Bearer 鉴权</td>
  </tr>
</table>
<p style="color:var(--muted);font-size:.92rem">下游 stdio 与 HTTP <strong>可并发同时启用</strong>（共享一份 <span class="inline">Arc&lt;GatewayState&gt;</span>），但至少须启用一种。</p>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li>mcpgw 是 Cargo 虚拟工作区，crate 各司其职、依赖<strong>无环</strong>。</li>
    <li>越底层越纯：<span class="inline">catalog</span> 无兄弟依赖、<span class="inline">retrieval</span> 无 HTTP，检索栈的 HTTP 客户端只在 <span class="inline">embedder</span>。</li>
    <li>上游 / 下游两个方向都支持 stdio + HTTP，可并发启用。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  这套分层把「会爆炸的工具列表」收敛成对外<strong>恒定的 3 个元工具</strong>，
  而真正复杂的检索逻辑全部留在网关内部。因为检索内核（<span class="inline">retrieval</span>）不被 HTTP、配置或 CLI 绑死，
  它可以<strong>独立演进</strong>：从 BM25 到向量检索、再到混合检索，都不动对外契约。
</div>

<p style="margin-top:1.2rem">
👉 检索策略正是网关的<strong>核心可插拔件</strong>。下一部分我们就钻进去，看<strong>向量检索</strong>是怎么建在
<span class="inline">retrieval</span> 抽象之上、又如何在嵌入失败时透明降级回 BM25 的。
</p>
"""

# ---------------------------------------------------------------------------
LESSON_03 = r"""
<p class="lead">
把前两课串起来：我们跟随一次「客户端想调用某个上游工具」的完整数据流，
看 3 个元工具是如何接力把请求从客户端送到上游、再把结果带回来的。
</p>

<h2>一次调用的四步生命周期</h2>

<div class="vflow">
  <div class="step">
    <div class="num">1</div>
    <div class="sc">
      <h4>连接 &amp; 看到 3 个元工具</h4>
      <p>客户端连上 <span class="inline">downstream</span>（stdio 或 HTTP），调 <span class="inline">list_tools</span>，
        看到的永远是 <span class="inline">search_tools</span> / <span class="inline">get_tool_details</span> / <span class="inline">call_tool</span> 这 3 个元工具。</p>
    </div>
  </div>
  <div class="step">
    <div class="num">2</div>
    <div class="sc">
      <h4>search_tools("…") —— 检索候选</h4>
      <p><span class="inline">metatools</span> 在不可变的 <span class="inline">GatewaySnapshot</span> 上跑检索策略（BM25 或 Vector），
        返回一组按相关性排好序的候选 <span class="inline">ToolSummary</span>（限定名 + 描述）。</p>
    </div>
  </div>
  <div class="step">
    <div class="num">3</div>
    <div class="sc">
      <h4>get_tool_details(qualified_name) —— 看清入参</h4>
      <p>从 <span class="inline">catalog</span> 取出该工具的完整 <span class="inline">ToolDef</span>（含 input schema），
        让模型知道要传哪些参数、各是什么类型。</p>
    </div>
  </div>
  <div class="step">
    <div class="num">4</div>
    <div class="sc">
      <h4>call_tool(qualified_name, args) —— 路由执行</h4>
      <p><span class="inline">metatools</span> 经 catalog 查出该限定名对应的 <span class="inline">(server, tool)</span>
        ——<strong>绝不靠拆 <span class="inline">__</span> 去猜</strong>——再路由到 <span class="inline">upstream</span> 对应 handle 转发，带每调用超时。</p>
    </div>
  </div>
</div>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应：快照与重建</div>
  <p>检索（步骤 2/3/4 的读路径）始终发生在一份<strong>不可变快照</strong>上，所以<strong>读路径无锁</strong>：
    网关用 <span class="inline">ArcSwap&lt;GatewaySnapshot&gt;</span> 持有当前快照。
    当上游发来 <span class="inline">tools/list_changed</span> 时，后台 <span class="inline">rebuild_snapshot</span> 会
    <strong>build-then-swap</strong>（先在旁边重建好新快照，再原子换上），整个过程<strong>不阻塞</strong>正在进行的检索。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/metatools/src/tools.rs · search_tools / get_tool_details / call_tool</span></div>
<pre><span class="cm">// 读路径：在不可变快照上检索，无需加锁</span>
<span class="kw">pub async fn</span> <span class="fn">search_tools</span>(snap: &amp;GatewaySnapshot, query: &amp;str, top_k: usize)
    -&gt; Vec&lt;ToolSummary&gt; { snap.strategy.<span class="fn">search</span>(query, top_k).<span class="kw">await</span> ... }

<span class="cm">// 路由：经 catalog 查 (server, tool)，绝不拆 "__"</span>
<span class="kw">pub async fn</span> <span class="fn">call_tool</span>(snap, registry, name, arguments) -&gt; Result&lt;_, MetaError&gt; {
    <span class="kw">let</span> def = snap.catalog.<span class="fn">get</span>(name)?;          <span class="cm">// (server, tool)</span>
    <span class="kw">let</span> handle = registry.<span class="fn">get</span>(&amp;def.server)?;      <span class="cm">// 找到对应上游</span>
    handle.<span class="fn">call_tool</span>(&amp;def.name, arguments).<span class="kw">await</span>  <span class="cm">// 转发（带超时）</span>
}</pre>
  </div>
  <p style="color:var(--muted);font-size:.9rem;margin-top:.4rem">快照重建逻辑见 <span class="inline">crates/gateway/src/lib.rs · GatewayState::rebuild_snapshot</span>。</p>
</div>

<div class="card warn">
  <div class="tag">⚠️ 注意</div>
  网关的<strong>日志全部走 stderr</strong>，<strong>stdout 专门留给 MCP 协议帧</strong>。
  在 stdio 传输下，往 stdout 打任何普通日志都会污染协议流、让客户端解析失败——这是一条硬规则。
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <p>记住这条三步心智模型即可：</p>
  <div class="flow">
    <div class="node"><div class="nt">检索</div><div class="nd">search_tools</div></div>
    <div class="arrow">→</div>
    <div class="node"><div class="nt">详情</div><div class="nd">get_tool_details</div></div>
    <div class="arrow">→</div>
    <div class="node"><div class="nt">执行</div><div class="nd">call_tool</div></div>
  </div>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  这正是「渐进式披露」在<strong>数据流</strong>层面的体现：无论上游有多少工具、如何增删，
  LLM 永远只面对 <strong>3 个稳定入口</strong>。复杂度（检索排序、快照重建、路由转发、超时隔离）
  全被收进网关内部，对外暴露的契约始终不变。
</div>
"""
