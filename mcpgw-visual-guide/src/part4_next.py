"""Part 4 — Hybrid retrieval (RRF), lesson 15. Written through for M2-B.

Every factual claim is anchored to the M2-B source tree:

  - HybridStrategy { bm25, vector, doc_count }, new(embedder), index indexes both
    arms + records doc_count, search runs both at full depth then rrf_fuse:
    crates/retrieval/src/hybrid.rs
  - rrf_fuse: sum 1/(RRF_K + rank) by qualified_name, sort score-desc/name-asc,
    truncate top_k; RRF_K = 60: crates/retrieval/src/hybrid.rs
  - build_strategy("hybrid", Some) -> HybridStrategy; ("hybrid", None) ->
    StrategyError::EmbedderRequired: crates/retrieval/src/lib.rs
  - config::validate requires [retrieval.vector] for strategy in {vector, hybrid}:
    crates/config/src/lib.rs
  - build_embedder builds an embedder for "vector" | "hybrid":
    crates/mcpgw/src/main.rs
  - Default strategy stays "bm25" (hybrid is opt-in, needs an embedder).
"""

# ---------------------------------------------------------------------------
LESSON_15 = r"""
<p class="lead">
前面我们见过两路检索：<strong>BM25</strong>（字面——查询和工具描述<strong>共享词</strong>才命中）与
<strong>向量</strong>（语义——意思相近就命中）。它们各有盲区：BM25 漏掉「换了说法」的工具，向量则可能
被字面精确的查询带偏。<strong>Hybrid</strong> 用 <strong>RRF（Reciprocal Rank Fusion，倒数排名融合）</strong>
把两路的<strong>排名</strong>合并到一起，<strong>取长补短</strong>。它是<strong>可选</strong>策略，
<strong>默认仍是 <span class="inline">bm25</span></strong>。
</p>

<div class="card analogy">
  <div class="tag">🔌 生活类比</div>
  你同时问<strong>两位图书管理员</strong>：一位<strong>按书名关键词</strong>找（BM25），一位<strong>凭理解推荐</strong>（向量）。
  两人各给你一份<strong>排名</strong>。问题是——他们打的「分」根本<strong>不可比</strong>（一个数词频、一个算余弦）。
  聪明的做法不是比分数，而是看<strong>名次</strong>：<strong>谁在两份榜单里都靠前，谁就最该被信任。</strong>
  这正是 RRF 干的事。
</div>

<h2>为什么按「名次」融合，而不是按「分数」</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p>BM25 的分（词频 × idf）和向量的分（余弦相似度）<strong>量纲完全不同</strong>，直接相加毫无意义。
  RRF 绕开这个问题：<strong>只看每个工具在各路排名里的名次 <span class="inline">rank</span></strong>（最好为 1），
  名次越靠前贡献越大：</p>
  <pre class="code"><span class="cm"># 融合分 = 把该工具在每一路里的「名次贡献」相加，k = 60 固定</span>
fused(doc) = Σ(路 L ∈ {BM25, 向量})   1 / (60 + rank_L(doc))</pre>
  <p>常数 <span class="inline">k=60</span> 是业界默认，用来<strong>压平</strong>头部名次的统治力（让第 1 名不至于碾压一切）。
  mcpgw 把它定死，不开放成配置——少一个旋钮，少一份漂移。</p>
</div>

<h2>一个三工具的算例</h2>
<p>设查询同时被两路检索。BM25 命中了 A、B（A 更靠前）；向量则把语义最近的 C 排第一，A、B 随后。
按 <span class="inline">1/(60+rank)</span> 累加：</p>

<table class="t">
  <tr><th>工具</th><th>BM25 名次</th><th>向量名次</th><th>RRF 融合分</th><th>最终名次</th></tr>
  <tr><td><strong>A</strong>（两路都靠前）</td><td>1</td><td>2</td><td>1/61 + 1/62 ≈ <strong>0.0325</strong></td><td>🥇 1</td></tr>
  <tr><td><strong>B</strong>（两路都有，略低）</td><td>2</td><td>3</td><td>1/62 + 1/63 ≈ <strong>0.0320</strong></td><td>🥈 2</td></tr>
  <tr><td><strong>C</strong>（仅语义命中）</td><td>—</td><td>1</td><td>1/61 ≈ <strong>0.0164</strong></td><td>🥉 3</td></tr>
</table>

<div class="card key">
  <div class="tag">✅ 这张表说明了两件事</div>
  <ul>
    <li><strong>共识胜出</strong>：A 在两路都靠前，融合后稳居第一——这是 hybrid 比单路更稳的来源。</li>
    <li><strong>召回增益</strong>：C <strong>没有任何字面命中</strong>（BM25 名次为「—」），却因语义相近被向量召回，
      仍然进入了最终结果。纯 BM25 永远给不出 C。</li>
  </ul>
</div>

<h2>两路的「不对称」是有意的</h2>

<div class="cols">
  <div class="col">
    <h4>BM25 一路</h4>
    <ul>
      <li><strong>只返回命中词的工具</strong>（分数 &gt; 0 才入榜）。</li>
      <li>没有共享词 → 直接不在榜上。</li>
    </ul>
  </div>
  <div class="col">
    <h4>向量一路</h4>
    <ul>
      <li><strong>给全部工具</strong>按余弦排名（即使相似度很低）。</li>
      <li>所以「仅语义相关」的工具也能通过这一路进入融合。</li>
    </ul>
  </div>
</div>
<p>正因为向量这一路<strong>覆盖全部工具</strong>，hybrid 才能把 BM25 漏掉的语义命中捞回来——这就是上表里 C 的来历。</p>

<h2>关键实现：全深度融合 + 复用现成两路</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">HybridStrategy</span> 不重新发明轮子——它<strong>内部组合</strong>一个现成的
  <span class="inline">Bm25Strategy</span> 和一个 <span class="inline">VectorStrategy</span>，检索时各取一份排名再融合。
  注意 <strong>子检索按 <span class="inline">doc_count</span>（全目录深度）而非 <span class="inline">top_k</span></strong>：
  若先各自截到 <span class="inline">top_k</span> 再融合，会丢掉「一边名次低、另一边名次高」的单边命中，破坏 RRF 的正确性。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/src/hybrid.rs · HybridStrategy::search + rrf_fuse</span></div>
<pre><span class="kw">async fn</span> <span class="fn">search</span>(&amp;self, query: &amp;<span class="kw">str</span>, top_k: <span class="kw">usize</span>) -&gt; Vec&lt;ScoredTool&gt; {
    <span class="kw">if</span> self.doc_count == 0 { <span class="kw">return</span> Vec::<span class="fn">new</span>(); }
    <span class="cm">// 关键：按 doc_count（全目录深度）跑两路，而非 top_k——</span>
    <span class="cm">// RRF 必须看到每个工具在各自排名里的「真实名次」</span>
    <span class="kw">let</span> lb = self.bm25.<span class="fn">search</span>(query, self.doc_count).<span class="kw">await</span>;   <span class="cm">// 字面一路</span>
    <span class="kw">let</span> lv = self.vector.<span class="fn">search</span>(query, self.doc_count).<span class="kw">await</span>; <span class="cm">// 语义一路</span>
    <span class="fn">rrf_fuse</span>(&amp;[lb, lv], top_k)
}

<span class="kw">fn</span> <span class="fn">rrf_fuse</span>(lists: &amp;[Vec&lt;ScoredTool&gt;], top_k: <span class="kw">usize</span>) -&gt; Vec&lt;ScoredTool&gt; {
    <span class="kw">let mut</span> fused: HashMap&lt;String, (f32, String)&gt; = HashMap::<span class="fn">new</span>();
    <span class="kw">for</span> list <span class="kw">in</span> lists {
        <span class="kw">for</span> (i, hit) <span class="kw">in</span> list.<span class="fn">iter</span>().<span class="fn">enumerate</span>() {
            <span class="kw">let</span> rank = (i + 1) <span class="kw">as</span> f32;
            <span class="cm">// RRF_K = 60，按名次把贡献累加到该工具名下</span>
            fused.<span class="fn">entry</span>(hit.qualified_name.<span class="fn">clone</span>())
                 .<span class="fn">or_insert_with</span>(|| (0.0, hit.description.<span class="fn">clone</span>()))
                 .0 += 1.0 / (RRF_K + rank);
        }
    }
    <span class="cm">// 按融合分降序、qualified_name 升序排序（确定性 tie-break），截断 top_k</span>
}</pre>
  </div>
</div>

<h2>嵌入坏了怎么办？自动退化≈纯 BM25</h2>

<div class="card key">
  <div class="tag">🛟 降级自愈（无需额外代码）</div>
  <p>还记得上一部分：<span class="inline">VectorStrategy</span> 在嵌入失败时会<strong>透明回落到它内置的 BM25</strong>。
  放进 hybrid 里，这条性质<strong>免费复用</strong>了：</p>
  <ul>
    <li>嵌入服务挂了 → 向量这一路返回的其实是 BM25 排名；</li>
    <li>于是 RRF 融合的<strong>两份榜单≈同一份 BM25 排名</strong> → 融合后名次单调一致；</li>
    <li>结果：<strong>hybrid 平滑退化≈纯 BM25</strong>，不会硬失败，也不需要在 HybridStrategy 里再写一个降级标志。</li>
  </ul>
</div>

<h2>怎么开启（以及为什么默认还是 bm25）</h2>

<div class="card detail">
  <div class="tag">🔬 装配 / 代码对应</div>
  <p>hybrid 和 vector 一样<strong>需要一个 embedder</strong>（云端嵌入 + API Key），因此三处校验保持一致：</p>
  <ul>
    <li><span class="inline">config::validate</span>：<span class="inline">strategy ∈ {vector, hybrid}</span> 时<strong>必须</strong>有 <span class="inline">[retrieval.vector]</span> 段。</li>
    <li><span class="inline">build_strategy("hybrid", Some(e))</span> → <span class="inline">HybridStrategy</span>；缺 embedder（<span class="inline">None</span>）→ <span class="inline">StrategyError::EmbedderRequired</span>。</li>
    <li><span class="inline">build_embedder</span>：对 <span class="inline">"vector" | "hybrid"</span> 都会从 <span class="inline">api_key_env</span> 启动期读密钥、建 embedder。</li>
  </ul>
  <pre class="code"><span class="cm"># config.toml —— 开启 hybrid</span>
[retrieval]
strategy = <span class="st">"hybrid"</span>          <span class="cm"># 默认是 "bm25"；这里显式切到 hybrid</span>
[retrieval.vector]
model       = <span class="st">"text-embedding-3-small"</span>
api_key_env = <span class="st">"OPENAI_API_KEY"</span>   <span class="cm"># 只引用环境变量名，绝不写明文密钥</span></pre>
</div>

<div class="card warn">
  <div class="tag">⚠️ 易错点</div>
  hybrid <strong>离不开联网 embedder</strong>。离线的 <span class="inline">mcpgw search</span> CLI <strong>不注入 embedder</strong>，
  所以对 <span class="inline">strategy="vector"/"hybrid"</span> 会直接报 <span class="inline">EmbedderRequired</span>——
  它们只在 <span class="inline">serve</span>（在线网关，启动期建好 embedder）下才真正生效。
</div>

<div class="card spark">
  <div class="tag">💡 为什么默认不是 hybrid</div>
  路线图原本设想「默认 BM25+向量混合」。但 hybrid <strong>离不开云端 embedder（要 API Key、要联网）</strong>，
  做默认就意味着<strong>开箱即跑会失败</strong>。所以最终决定：<strong>默认仍是零依赖、离线可用的 <span class="inline">bm25</span></strong>，
  hybrid 作为 <strong>opt-in</strong>——配了 <span class="inline">[retrieval.vector]</span> 才启用。零配置体验与语义能力，二者兼得。
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li><strong>RRF 按名次融合</strong>：<span class="inline">Σ 1/(60+rank)</span>，绕开 BM25 分与余弦分不可比的问题。</li>
    <li><strong>取长补短</strong>：两路都靠前的工具胜出；仅语义命中的工具靠向量这一路<strong>被召回</strong>。</li>
    <li><strong>全深度融合</strong>：子检索按 <span class="inline">doc_count</span> 而非 <span class="inline">top_k</span>，RRF 才能看到真实名次。</li>
    <li><strong>降级自愈</strong>：嵌入坏了，两路≈同一 BM25，hybrid 退化≈纯 BM25——免费复用 VectorStrategy 的降级。</li>
    <li><strong>opt-in</strong>：需 <span class="inline">[retrieval.vector]</span>；<strong>默认仍是 <span class="inline">bm25</span></strong>。</li>
  </ul>
</div>

<div class="card macro">
  <div class="tag">🗺️ 回到全局</div>
  到这里，检索这条线就完整了：<strong>BM25（字面）</strong>、<strong>向量（语义）</strong>、以及把两者
  <strong>RRF 融合</strong>的 <strong>Hybrid</strong>。它们都藏在网关的三个元工具之后，对客户端透明——
  客户端永远只看到 <span class="inline">search_tools</span> / <span class="inline">get_tool_details</span> / <span class="inline">call_tool</span>，
  而<strong>用哪种检索策略，只是一行配置的事</strong>。
</div>
"""
