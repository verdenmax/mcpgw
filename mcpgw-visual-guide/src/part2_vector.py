"""Part 2 (vector retrieval deep dive): lessons 04-08.

This task writes 04 (overview) and 05 (Embedder & OpenAiEmbedder) through.
Lessons 06-08 are appended to THIS file by later tasks, so keep each lesson as a
clearly separated `LESSON_XX = r\"\"\"…\"\"\"` block.

Every factual claim is anchored to the M2-A source tree:

  - Embedder trait + EmbedError + MockEmbedder: crates/retrieval/src/embedder.rs
  - OpenAiEmbedder (POST {base_url}/embeddings, bearer_auth, sort-by-index,
    count/index/dim validation, ≤500-char body snippet, empty input short-circuit,
    sole reqwest 0.13 user): crates/embedder/src/lib.rs
  - VectorStrategy (built-in Bm25Strategy + degraded flag, index builds BM25 first
    then tries embed, search falls back to BM25): crates/retrieval/src/vector.rs
  - Semantic-gain smoke ("communicate with my team" -> slack__post_message):
    crates/mcpgw/tests/smoke_vector_real.rs
"""

# ---------------------------------------------------------------------------
LESSON_04 = r"""
<p class="lead">
上一部分我们见过 <strong>BM25</strong>：它是<strong>字面匹配</strong>——查询和工具描述<strong>共享词</strong>才会命中。
但人说话千变万化：「联系我的团队」和工具描述里的 <span class="inline">post message</span> 一个共享词都没有，BM25 就召回为空。
<strong>向量检索</strong>解决的正是这件事：把文本变成向量，<strong>意思相近</strong>就命中，哪怕一个字都不重合。
mcpgw 把检索做成<strong>可插拔策略</strong>，而向量策略<strong>内置 BM25 作透明降级</strong>——嵌入服务坏了也不会硬失败。
</p>

<div class="card analogy">
  <div class="tag">🔌 生活类比</div>
  <strong>BM25</strong> 像「按书名里的关键词找书」：你说的词必须正好印在书名上，才找得到。
  <strong>向量检索</strong>像「告诉图书管理员你想干什么，他凭<strong>理解</strong>给你推荐」——
  你说「我想给团队发个通知」，他自然递来《如何用 Slack 发消息》，哪怕书名里根本没有「通知」二字。
</div>

<h2>两种检索机制的对比</h2>

<div class="cols">
  <div class="col">
    <h4>BM25（字面 / 稀疏）</h4>
    <ul>
      <li><strong>命中机制</strong>：查询词与文档<strong>共享 token</strong> 才有分。</li>
      <li><strong>同义/改写鲁棒性</strong>：弱——换个说法就可能<strong>完全召回不到</strong>。</li>
      <li><strong>外部服务</strong>：不需要，纯本地算词频。</li>
      <li><strong>离线可用</strong>：✅ 永远可用，无网络依赖。</li>
    </ul>
  </div>
  <div class="col">
    <h4>向量（语义 / 稠密）</h4>
    <ul>
      <li><strong>命中机制</strong>：靠<strong>余弦相似度</strong>，意思接近就高分。</li>
      <li><strong>同义/改写鲁棒性</strong>：强——换说法、近义词依然能命中。</li>
      <li><strong>外部服务</strong>：需要一个 <strong>embedding 服务</strong>把文本转成向量。</li>
      <li><strong>离线可用</strong>：⚠️ 取决于服务可达；不可达时就要降级。</li>
    </ul>
  </div>
</div>

<h2>关键设计：透明降级</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">VectorStrategy</span> 并不是「纯向量」——它同时持有一个 <span class="inline">embedder</span>、
  一个<strong>内置的 <span class="inline">Bm25Strategy</span></strong>，以及一个 <span class="inline">degraded</span> 标志：</p>
  <ul>
    <li><strong>index</strong>：<strong>总是先把 BM25 索引建好</strong>，再尝试对整个工具目录做一次批量嵌入；
      嵌入失败（或返回向量数量不对）就清空向量、把 <span class="inline">degraded</span> 置 <span class="inline">true</span>。</li>
    <li><strong>search</strong>：当 <span class="inline">degraded</span>、<strong>没有任何向量</strong>、或<strong>单次查询嵌入失败</strong>时，
      直接回落到 BM25 的结果。</li>
  </ul>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/src/vector.rs · VectorStrategy::{index,search}</span></div>
<pre><span class="kw">async fn</span> <span class="fn">index</span>(&amp;<span class="kw">mut</span> self, catalog: &amp;Catalog) {
    <span class="cm">// 无论如何先把 BM25 兜底索引建好</span>
    self.bm25 = Bm25Strategy::new();
    self.bm25.<span class="fn">index</span>(catalog).<span class="kw">await</span>;

    <span class="kw">match</span> self.embedder.<span class="fn">embed</span>(&amp;texts).<span class="kw">await</span> {
        Ok(vecs) <span class="kw">if</span> vecs.len() == tools.len() =&gt; { <span class="cm">/* 存归一化向量 */</span> self.degraded = <span class="kw">false</span>; }
        _ =&gt; { self.vectors.<span class="fn">clear</span>(); self.degraded = <span class="kw">true</span>; } <span class="cm">// 失败/数量不符 → 降级</span>
    }
}

<span class="kw">async fn</span> <span class="fn">search</span>(&amp;self, query: &amp;<span class="kw">str</span>, top_k: <span class="kw">usize</span>) -&gt; Vec&lt;ScoredTool&gt; {
    <span class="kw">if</span> self.degraded || self.vectors.<span class="fn">is_empty</span>() {
        <span class="kw">return</span> self.bm25.<span class="fn">search</span>(query, top_k).<span class="kw">await</span>; <span class="cm">// 透明回落 BM25</span>
    }
    <span class="kw">let</span> qv = <span class="kw">match</span> self.embedder.<span class="fn">embed</span>(&amp;[query.<span class="fn">to_string</span>()]).<span class="kw">await</span> {
        Ok(<span class="kw">mut</span> v) =&gt; <span class="fn">normalize</span>(v.<span class="fn">remove</span>(0)),
        Err(_) =&gt; <span class="kw">return</span> self.bm25.<span class="fn">search</span>(query, top_k).<span class="kw">await</span>, <span class="cm">// 单次查询嵌入失败也回落</span>
    };
    <span class="cm">// …对每个工具向量算 dot(qv, v) 余弦打分、排序、截断 top_k…</span>
}</pre>
  </div>
</div>

<h2>语义增益：一个 BM25 救不了的查询</h2>
<p>这是真实的门控冒烟用例（<span class="inline">crates/mcpgw/tests/smoke_vector_real.rs</span>）。查询
<span class="inline">"communicate with my team"</span> 与<strong>任何</strong>工具描述都<strong>没有共享词</strong>：</p>

<div class="flow">
  <div class="node"><div class="nt">查询 "communicate with my team"</div><div class="nd">和所有工具描述零共享词</div></div>
  <div class="arrow">→</div>
  <div class="node"><div class="nt">BM25</div><div class="nd">召回为空（没有命中词）</div></div>
  <div class="arrow">→</div>
  <div class="node hl"><div class="nt">向量检索</div><div class="nd">把 slack__post_message 排第一</div></div>
</div>

<table class="t">
  <tr><th>检索策略</th><th>对 "communicate with my team" 的结果</th></tr>
  <tr><td><strong>BM25（字面）</strong></td><td>空——没有任何共享词可命中</td></tr>
  <tr><td><strong>Vector（语义）</strong></td><td><span class="inline">slack__post_message</span> 排在第一</td></tr>
</table>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li>检索是<strong>可插拔策略</strong>：默认仍是 <span class="inline">bm25</span>，按需切换到 <span class="inline">vector</span>。</li>
    <li>向量检索<strong>语义匹配</strong>：意思相近即命中，能召回 BM25 完全漏掉的工具。</li>
    <li>向量策略<strong>永不「硬失败」</strong>：embedder 一坏，就退回内置 BM25 的结果。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  降级是<strong>透明</strong>的——调用方拿到的永远是「尽力而为的最佳排序」：
  能嵌入就给语义结果，不能嵌入就给 BM25 结果，<strong>无需自己处理 embedder 故障</strong>，也无需关心当前到底走了哪条路。
</div>
"""

# ---------------------------------------------------------------------------
LESSON_05 = r"""
<p class="lead">
要做向量检索，第一步是把文本变成向量。mcpgw 用一个<strong>与厂商无关</strong>的 <span class="inline">Embedder</span> trait
把「怎么变」这件事抽象掉，真实的 HTTP 实现放在<strong>独立的 <span class="inline">embedder</span> crate</strong>。
于是 <span class="inline">retrieval</span> 这个检索内核<strong>不引入任何 HTTP 依赖</strong>，可以在无网络下编译和测试。
</p>

<h2>抽象层：Embedder trait</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p>trait 只有两个方法：<span class="inline">embed</span> 把<strong>一批文本各转成一个向量</strong>（<strong>顺序一一对应</strong>、
  <strong>all-or-nothing</strong>——要么整批成功，要么报错），<span class="inline">dim</span> 返回期望维度用于体检。
  错误类型 <span class="inline">EmbedError</span> 也是<strong>与 provider 无关</strong>的，所以检索内核完全不知道背后是谁、用不用 HTTP。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/src/embedder.rs · Embedder / EmbedError</span></div>
<pre><span class="cm">// provider 无关，于是 retrieval 不需要任何 HTTP 依赖</span>
<span class="kw">pub enum</span> EmbedError {
    Provider(String),                         <span class="cm">// 厂商/网络/解码等失败</span>
    Dimension { expected: <span class="kw">usize</span>, got: <span class="kw">usize</span> }, <span class="cm">// 维度不符</span>
}

<span class="kw">pub trait</span> Embedder: Send + Sync {
    <span class="cm">// 一批文本 → 各一个向量，顺序对应；要么全成功要么 Err</span>
    <span class="kw">async fn</span> <span class="fn">embed</span>(&amp;self, texts: &amp;[String]) -&gt; Result&lt;Vec&lt;Vec&lt;f32&gt;&gt;, EmbedError&gt;;
    <span class="kw">fn</span> <span class="fn">dim</span>(&amp;self) -&gt; <span class="kw">usize</span>; <span class="cm">// 期望维度，用于校验</span>
}</pre>
  </div>
</div>

<h2>真实实现：OpenAiEmbedder</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">OpenAiEmbedder</span>（<span class="inline">crates/embedder/src/lib.rs</span>）对接任何 OpenAI 兼容的
  <span class="inline">/embeddings</span> 端点（OpenAI 本体，或 Ollama / LM Studio / vLLM 等同形状的本地服务）。它的 <span class="inline">embed</span> 做这几件事：</p>
  <ul>
    <li><strong>请求</strong>：<span class="inline">POST {base_url}/embeddings</span>，用 <span class="inline">bearer_auth(api_key)</span> 带上密钥，
      body 是 <span class="inline">{model, input}</span>。</li>
    <li><strong>排序</strong>：服务器可能乱序返回，所以按响应里的 <span class="inline">index</span> <strong>排序</strong>，
      再校验<strong>数量</strong>与 <span class="inline">index</span> 是否<strong>从 0 连续</strong>；设了 <span class="inline">dim</span> 还会逐条校验维度。</li>
    <li><strong>错误信息</strong>：非 2xx 时把<strong>响应体截断片段（最多 500 字符）</strong>放进错误，便于排错；
      但<strong>绝不回显 Authorization</strong>。</li>
    <li><strong>空输入短路</strong>：<span class="inline">texts</span> 为空直接返回 <span class="inline">Ok(vec![])</span>，不发请求。</li>
    <li>它是整个 workspace 里<strong>唯一</strong>依赖 <strong>reqwest 0.13（rustls）</strong>的 crate，HTTP 被隔离在这一处。</li>
  </ul>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/embedder/src/lib.rs · OpenAiEmbedder::embed</span></div>
<pre><span class="kw">async fn</span> <span class="fn">embed</span>(&amp;self, texts: &amp;[String]) -&gt; Result&lt;Vec&lt;Vec&lt;f32&gt;&gt;, EmbedError&gt; {
    <span class="kw">if</span> texts.<span class="fn">is_empty</span>() { <span class="kw">return</span> Ok(Vec::new()); } <span class="cm">// 空输入短路</span>
    <span class="kw">let</span> resp = self.client
        .<span class="fn">post</span>(&amp;<span class="fn">format!</span>(<span class="st">"{}/embeddings"</span>, self.base_url))
        .<span class="fn">bearer_auth</span>(&amp;self.api_key)                 <span class="cm">// Bearer 密钥</span>
        .<span class="fn">json</span>(&amp;<span class="fn">json!</span>({ <span class="st">"model"</span>: self.model, <span class="st">"input"</span>: texts }))
        .<span class="fn">send</span>().<span class="kw">await</span>?;
    <span class="kw">if</span> !resp.<span class="fn">status</span>().<span class="fn">is_success</span>() {
        <span class="kw">let</span> snippet: String = body.<span class="fn">chars</span>().<span class="fn">take</span>(500).<span class="fn">collect</span>(); <span class="cm">// ≤500 字符，绝不含 Authorization</span>
        <span class="kw">return</span> Err(EmbedError::Provider(<span class="fn">format!</span>(<span class="st">"HTTP {code} …: {snippet}"</span>)));
    }
    <span class="kw">let mut</span> data = parsed.data;
    data.<span class="fn">sort_by_key</span>(|d| d.index);                   <span class="cm">// 按 index 还原输入顺序</span>
    <span class="cm">// …校验 data.len()==texts.len()、index 连续、可选 dim 一致…</span>
}</pre>
  </div>
</div>

<div class="card warn">
  <div class="tag">⚠️ 注意：密钥</div>
  构造 <span class="inline">OpenAiEmbedder</span> 时传入的 <span class="inline">api_key</span> 是<strong>真实的 token 值</strong>，
  而这个值来自<strong>环境变量</strong>。但配置文件里存的是<strong>env 变量名</strong>（不是值本身，详见第 08 课）。
  于是任何错误信息只会提到<strong>变量名</strong>，绝不打印密钥值——HTTP 错误片段也刻意只截响应体、<strong>不含 Authorization 头</strong>。
</div>

<h2>测试替身：MockEmbedder</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">MockEmbedder</span>（在 <span class="inline">retrieval</span> 的 <span class="inline">testkit</span> feature 下）让测试<strong>不依赖网络</strong>：
  它把每个 token 用 <strong>FNV 哈希分桶</strong>，在对应桶 +1，生成<strong>确定性的伪向量</strong>——
  共享 token 越多的文本，余弦相似度越高，所以语义检索行为可被稳定验证。它还暴露
  <span class="inline">calls</span>（embed 被调用次数）和 <span class="inline">seen</span>（见过哪些文本），供<strong>缓存断言</strong>使用（第 06 课会用到）；
  另有 <span class="inline">MockEmbedder::failing</span> 专门让 <span class="inline">embed</span> 必报错，用来驱动降级测试。</p>
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li><span class="inline">Embedder</span> trait 把「文本→向量」抽象成两个方法：<span class="inline">embed</span>（批量、顺序对应、all-or-nothing）+ <span class="inline">dim</span>。</li>
    <li><span class="inline">EmbedError</span> 只有 <span class="inline">Provider(String)</span> 与 <span class="inline">Dimension{expected,got}</span> 两种，<strong>与厂商无关</strong>。</li>
    <li><span class="inline">OpenAiEmbedder</span> 是<strong>唯一</strong>带 <strong>reqwest 0.13</strong> 的 crate：发请求、按 index 排序校验、错误信息含截断响应体但绝不回显密钥。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  这条 trait 边界把「HTTP / 厂商细节」与「检索逻辑」<strong>彻底解耦</strong>：
  HTTP 只活在 <span class="inline">embedder</span> 这一个 crate 里，于是 <span class="inline">retrieval</span> 可以在<strong>无网络</strong>下编译、用 <span class="inline">MockEmbedder</span> 确定性地测试，
  换厂商（OpenAI / 本地服务）也只换一个实现，检索内核一行都不用动。
</div>
"""
