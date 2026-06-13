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

# ---------------------------------------------------------------------------
LESSON_06 = r"""
<p class="lead">
每次快照重建，都要给<strong>整个工具目录</strong>重新算一遍嵌入——可工具大多<strong>压根没变</strong>，重算就是白白花钱、白等延迟。
<span class="inline">CachingEmbedder</span> 是一个<strong>装饰器</strong>：它包在任意 <span class="inline">Embedder</span> 外面，按<strong>文本内容的哈希</strong>记住算过的向量，
只把<strong>缓存未命中</strong>的文本转发给内层 embedder。见过的文本，直接复用旧向量。
</p>

<div class="card analogy">
  <div class="tag">🔌 生活类比</div>
  像<strong>背单词卡片</strong>：见过的词翻面直接念出答案，只有<strong>没见过的生词</strong>才去翻词典。
  词典（内层 embedder）很贵又慢，卡片堆（缓存）又快又免费——所以能不翻就不翻。
</div>

<h2>怎么记：用内容哈希当 key</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p>缓存的 key 是<strong>文本内容的 64-bit 哈希</strong>，用 <strong>FNV-1a</strong> 算法（<span class="inline">hash_text</span>：初值
  <span class="inline">0xcbf29ce484222325</span>，每个字节异或后乘 <span class="inline">0x100000001b3</span>）。命中就直接复用缓存里的向量，
  未命中才会真正去嵌入。<span class="inline">embed</span> 的实现分<strong>三段</strong>：先收集 miss、只嵌 miss、再按原序重组。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/src/caching.rs · CachingEmbedder::embed</span></div>
<pre><span class="kw">fn</span> <span class="fn">hash_text</span>(text: &amp;<span class="kw">str</span>) -&gt; <span class="kw">u64</span> {       <span class="cm">// FNV-1a</span>
    <span class="kw">let mut</span> h: <span class="kw">u64</span> = <span class="st">0xcbf29ce484222325</span>;
    <span class="kw">for</span> b <span class="kw">in</span> text.<span class="fn">as_bytes</span>() { h ^= *b <span class="kw">as</span> <span class="kw">u64</span>; h = h.<span class="fn">wrapping_mul</span>(<span class="st">0x100000001b3</span>); }
    h
}

<span class="kw">async fn</span> <span class="fn">embed</span>(&amp;self, texts: &amp;[String]) -&gt; Result&lt;Vec&lt;Vec&lt;f32&gt;&gt;, EmbedError&gt; {
    <span class="kw">let</span> hashes: Vec&lt;<span class="kw">u64</span>&gt; = texts.<span class="fn">iter</span>().<span class="fn">map</span>(|t| <span class="fn">hash_text</span>(t)).<span class="fn">collect</span>();

    <span class="cm">// ① 持锁：收集去重的 cache-miss 文本，保留首次出现顺序</span>
    <span class="kw">let mut</span> miss_texts = Vec::new();
    { <span class="kw">let</span> cache = self.cache.<span class="fn">lock</span>().<span class="fn">unwrap</span>();
      <span class="kw">for</span> (h, t) <span class="kw">in</span> hashes.<span class="fn">iter</span>().<span class="fn">zip</span>(texts) {
          <span class="kw">if</span> !cache.<span class="fn">contains_key</span>(h) &amp;&amp; miss_seen.<span class="fn">insert</span>(*h) { miss_texts.<span class="fn">push</span>(t.<span class="fn">clone</span>()); } } }

    <span class="cm">// ② 不持锁：全命中就跳过；否则只嵌 miss，再持锁写回</span>
    <span class="kw">if</span> !miss_texts.<span class="fn">is_empty</span>() {
        <span class="kw">let</span> embedded = self.inner.<span class="fn">embed</span>(&amp;miss_texts).<span class="kw">await</span>?;   <span class="cm">// ← 此处不持锁</span>
        <span class="kw">let mut</span> cache = self.cache.<span class="fn">lock</span>().<span class="fn">unwrap</span>();
        <span class="kw">for</span> (t, v) <span class="kw">in</span> miss_texts.<span class="fn">iter</span>().<span class="fn">zip</span>(embedded) { cache.<span class="fn">insert</span>(<span class="fn">hash_text</span>(t), Arc::<span class="fn">from</span>(/* v */)); }
    }

    <span class="cm">// ③ 持锁：按原始输入顺序重组输出</span>
    <span class="kw">let</span> cache = self.cache.<span class="fn">lock</span>().<span class="fn">unwrap</span>();
    Ok(hashes.<span class="fn">iter</span>().<span class="fn">map</span>(|h| cache.<span class="fn">get</span>(h).<span class="fn">expect</span>(<span class="st">"just inserted/hit"</span>).<span class="fn">to_vec</span>()).<span class="fn">collect</span>())
}</pre>
  </div>
</div>

<h2>关键纪律：绝不跨 .await 持锁</h2>

<div class="card warn">
  <div class="tag">⚠️ 锁纪律</div>
  <p>缓存用的是 <span class="inline">std::sync::Mutex</span>（同步锁），而 <span class="inline">embed</span> 是 <span class="inline">async</span> 的。
  实现把工作切成<strong>三个互不相交的临界区</strong>，<strong>绝不把锁跨 <span class="inline">.await</span> 持有</strong>：</p>
  <div class="vflow">
    <div class="step"><div class="num">1</div><div class="sc"><h4>持锁 · 读 miss</h4>
      <p>锁住缓存，挑出去重的未命中文本，<strong>出了 <span class="inline">{}</span> 作用域立刻解锁</strong>。</p></div></div>
    <div class="step"><div class="num">2</div><div class="sc"><h4>不持锁 · 只嵌 miss</h4>
      <p><span class="inline">inner.embed(&amp;miss_texts).await</span> 在这里发生——<strong>此时手里没有锁</strong>，再持锁把结果写回。</p></div></div>
    <div class="step"><div class="num">3</div><div class="sc"><h4>持锁 · 重组</h4>
      <p>重新锁住缓存，按原始输入顺序拼出每个文本对应的向量。</p></div></div>
  </div>
  <p>为什么重要？<span class="inline">std::sync::Mutex</span> 的守卫<strong>不是 <span class="inline">Send</span></strong>，跨 <span class="inline">.await</span> 持有它
  会让 future 无法在多线程执行器间迁移，更糟的是<strong>持锁等待网络</strong>会把别的任务全堵死。把唯一的 <span class="inline">.await</span> 夹在两次解锁之间，正好避开这一切。</p>
</div>

<h2>为什么那个 .expect 永不 panic</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / insert-only 不变量</div>
  <p>第三段重组里的 <span class="inline">.expect("just inserted/hit")</span> <strong>永远不会触发 panic</strong>，靠的是一条
  <strong>insert-only（只插入、从不淘汰）不变量</strong>：到这一步，每个 key 要么<strong>本来就命中</strong>、要么<strong>刚刚在第二段被插入</strong>，
  而缓存<strong>从不删除、从不淘汰</strong>，所以此刻<strong>每个 key 必然存在</strong>。
  源码里有注释明确写道：将来若加 <span class="inline">LRU</span>/<span class="inline">TTL</span> 淘汰策略，就必须在<strong>这一步重新处理 miss</strong>，否则这个 <span class="inline">.expect</span> 就会变成真正的 bug。</p>
</div>

<h2>缓存跨多次重建存活</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 跨 rebuild 持久</div>
  <p>在装配层，<span class="inline">CachingEmbedder</span> 只被<strong>构造一次</strong>（细节见<strong>第 08 课</strong>），于是同一个缓存会<strong>跨多次
  <span class="inline">rebuild_snapshot</span> 存活</strong>——没变的工具<strong>不会被重复嵌入</strong>。
  有门控测试用 <span class="inline">MockEmbedder.calls</span> 断言这一点：<strong>第二次重建时内层调用次数不再增长</strong>，证明命中全部走了缓存。</p>
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li><span class="inline">CachingEmbedder</span> 是<strong>装饰器</strong>：按 <strong>FNV-1a</strong> 内容哈希记忆向量，只转发 cache-miss 给内层。</li>
    <li><strong>三段式、绝不跨 <span class="inline">.await</span> 持锁</strong>：读 miss → 不持锁嵌入 → 重组，全程不阻塞异步执行器。</li>
    <li>重组的 <span class="inline">.expect</span> 在 <strong>insert-only 不变量</strong>下永不 panic；加淘汰策略必须重写此处。</li>
    <li>缓存<strong>跨 rebuild 存活</strong>，未变工具<strong>零重复嵌入</strong>，省钱又省延迟。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  装饰器模式把「<strong>省钱省延迟</strong>」这件事<strong>正交地</strong>叠加在<strong>任意</strong> <span class="inline">Embedder</span> 之上：
  内层是 OpenAI 还是本地服务、是 Mock 还是别的实现，统统无所谓；上层的 <span class="inline">VectorStrategy</span> 也<strong>完全无感</strong>——
  它拿到的还是一个普通 <span class="inline">Embedder</span>，缓存这层对它彻底透明。
</div>
"""

# ---------------------------------------------------------------------------
LESSON_07 = r"""
<p class="lead">
<span class="inline">VectorStrategy</span> 在云端嵌入之上做<strong>暴力余弦检索</strong>——工具目录很小，<strong>线性扫一遍</strong>就足够快，
不需要近似最近邻索引。而当嵌入服务不可用时，它会<strong>透明回落</strong>到内置的 BM25，对上层完全不露痕迹。
</p>

<h2>数据结构</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">VectorStrategy</span> 持有四样东西：</p>
  <ul>
    <li><span class="inline">embedder: Arc&lt;dyn Embedder&gt;</span>——把文本转成向量（很可能外面还套了第 06 课的缓存）。</li>
    <li><span class="inline">bm25: Bm25Strategy</span>——<strong>内置的字面兜底</strong>策略。</li>
    <li><span class="inline">vectors: Vec&lt;(qualified_name, description, 归一化向量)&gt;</span>——降级时为空。</li>
    <li><span class="inline">degraded: bool</span>——是否已退化为纯 BM25。</li>
  </ul>
  <p>每个工具被嵌入的文本是 <span class="inline">tool_text</span> = <span class="inline">"{qualified_name}\n{description}"</span>（限定名 + 换行 + 描述）。</p>
</div>

<h2>index：先建兜底，再尝试嵌入</h2>

<div class="vflow">
  <div class="step"><div class="num">1</div><div class="sc"><h4>总是先建 BM25</h4>
    <p><span class="inline">self.bm25 = Bm25Strategy::new(); self.bm25.index(catalog).await;</span> ——
    <strong>无论嵌入成不成功，兜底索引先就位</strong>。</p></div></div>
  <div class="step"><div class="num">2</div><div class="sc"><h4>收集 tool_text 并批量嵌入</h4>
    <p>对目录里每个工具算 <span class="inline">tool_text</span>，再一次性 <span class="inline">embedder.embed(&amp;texts).await</span>。</p></div></div>
  <div class="step"><div class="num">3</div><div class="sc"><h4>数量匹配 → 存归一化向量</h4>
    <p><span class="inline">Ok(vecs)</span> 且 <span class="inline">vecs.len() == tools.len()</span>：把每个向量 <span class="inline">normalize</span> 后存入
    <span class="inline">vectors</span>，<span class="inline">degraded = false</span>。</p></div></div>
  <div class="step"><div class="num">4</div><div class="sc"><h4>数量不符 → 降级</h4>
    <p><span class="inline">Ok(vecs)</span> 但数量对不上：这违反 embedder 的 <strong>all-or-nothing / 顺序对应</strong>契约，
    强行 <span class="inline">zip</span> 会让向量与工具<strong>错位</strong>。于是<strong>清空 <span class="inline">vectors</span>、<span class="inline">degraded = true</span></strong>，
    宁可降级也不建错位索引。</p></div></div>
  <div class="step"><div class="num">5</div><div class="sc"><h4>Err → 降级</h4>
    <p>嵌入直接报错，同样清空向量、<span class="inline">degraded = true</span>，退回 BM25。</p></div></div>
</div>

<h2>search：余弦打分，单次失败也兜底</h2>

<div class="flow">
  <div class="node"><div class="nt">degraded 或 vectors 空？</div><div class="nd">是 → 直接 bm25.search</div></div>
  <div class="arrow">→</div>
  <div class="node"><div class="nt">嵌入 query</div><div class="nd">单次失败 → 回落 BM25</div></div>
  <div class="arrow">→</div>
  <div class="node"><div class="nt">余弦打分</div><div class="nd">dot(归一化 query, 归一化向量)</div></div>
  <div class="arrow">→</div>
  <div class="node hl"><div class="nt">排序 + 截断</div><div class="nd">score 降序, qualified_name 升序 → top_k</div></div>
</div>

<p>当 <span class="inline">degraded</span> 或 <span class="inline">vectors</span> 为空时，<span class="inline">search</span> 直接走
<span class="inline">bm25.search(query, top_k)</span>；否则先嵌入查询（<strong>这一次嵌入失败也回落 BM25</strong>），
对每个工具算 <span class="inline">dot(归一化 query, 归一化向量)</span> 即<strong>余弦相似度</strong>，
按 <span class="inline">score</span> <strong>降序</strong>、并列时按 <span class="inline">qualified_name</span> <strong>升序</strong>做<strong>稳定排序</strong>，最后截断到 <span class="inline">top_k</span>。</p>

<h2>零范数不会算出 NaN</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">normalize</span> 做 L2 归一化时，对<strong>零向量原样返回</strong>（不做除法），所以<strong>绝不会除以 0</strong>，
  余弦打分里也就<strong>永远不会冒出 NaN</strong>。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/src/vector.rs · normalize</span></div>
<pre><span class="cm">// L2 归一化（原地）；零向量原样返回——它和任何向量的余弦都是 0</span>
<span class="kw">fn</span> <span class="fn">normalize</span>(<span class="kw">mut</span> v: Vec&lt;f32&gt;) -&gt; Vec&lt;f32&gt; {
    <span class="kw">let</span> norm = v.<span class="fn">iter</span>().<span class="fn">map</span>(|x| x * x).<span class="fn">sum</span>::&lt;f32&gt;().<span class="fn">sqrt</span>();
    <span class="kw">if</span> norm &gt; <span class="st">0.0</span> {              <span class="cm">// ← 仅当范数 &gt; 0 才除，零向量跳过</span>
        <span class="kw">for</span> x <span class="kw">in</span> &amp;<span class="kw">mut</span> v { *x /= norm; }
    }
    v
}</pre>
  </div>
</div>

<div class="card warn">
  <div class="tag">⚠️ 双重降级的两条路径</div>
  <p>「降级到 BM25」会在<strong>两个时机</strong>各自独立发生，别混为一谈：</p>
  <ul>
    <li><strong>index-time（建索引时）</strong>：整批嵌入 <span class="inline">Err</span>，<strong>或</strong>返回向量<strong>数量不符</strong> →
      清空 <span class="inline">vectors</span>、<span class="inline">degraded = true</span>。此后所有查询都走 BM25。</li>
    <li><strong>query-time（单次查询时）</strong>：即便索引是好的，<strong>这一次</strong>查询的嵌入失败，也会<strong>临时</strong>回落到
      <span class="inline">bm25.search</span>——不改 <span class="inline">degraded</span>，下次查询仍会再试向量。</li>
  </ul>
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li><strong>余弦 = 归一化点积</strong>：向量都先 <span class="inline">normalize</span>，打分就是一个 <span class="inline">dot</span>。</li>
    <li><strong>稳定排序</strong>：<span class="inline">score</span> 降序 + <span class="inline">qualified_name</span> 升序，结果确定可复现，再截断 <span class="inline">top_k</span>。</li>
    <li><strong>永不 NaN</strong>：零向量原样返回，不除零。</li>
    <li><strong>永不硬失败</strong>：index 与 query 两处都能透明回落 BM25。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  <span class="inline">VectorStrategy</span> 把「<strong>语义检索</strong>」与「<strong>字面兜底</strong>」缝进<strong>同一个策略</strong>里，
  对上层而言它就只是一个普通的 <span class="inline">RetrievalStrategy</span>——调用方既不用判断嵌入服务是否健在，也不用自己拼装兜底逻辑。
</div>
"""

# ---------------------------------------------------------------------------
LESSON_08 = r"""
<p class="lead">
本课把前几课的零件<strong>接成一台机器</strong>：当配置选 <span class="inline">strategy = "vector"</span> 时，
启动期如何<strong>构造 embedder</strong>、把它<strong>注入 gateway</strong>，并在<strong>缺凭证时立刻失败</strong>（fail-fast）——
而不是等到第一次查询才崩。配置层负责<strong>声明意图</strong>，装配层负责<strong>一次性兑现可靠性约束</strong>。
</p>

<h2>build_strategy：按名字造策略</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">build_strategy</span> 只认一个<strong>字符串名字</strong>和一个<strong>可选 embedder</strong>，
  返回一个装箱的 <span class="inline">RetrievalStrategy</span>：</p>
  <ul>
    <li><span class="inline">"bm25"</span> —— <strong>无需 embedder</strong>，直接造 <span class="inline">Bm25Strategy</span>。</li>
    <li><span class="inline">"vector"</span> —— <strong>必须有</strong> embedder；没有就返回 <span class="inline">StrategyError::EmbedderRequired</span>。</li>
    <li><span class="inline">"hybrid"</span> / 其它未知名字 —— <span class="inline">StrategyError::NotImplemented</span>（留给 M2-B）。</li>
  </ul>
  <p>它故意只收 <span class="inline">&amp;str</span>（不依赖 <span class="inline">config</span> 类型），所以 <span class="inline">retrieval</span> 这个 crate 不必反向依赖配置层。
  另外提醒：<strong>默认策略仍是 <span class="inline">bm25</span></strong>，向量是显式选择项。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/retrieval/src/lib.rs · build_strategy</span></div>
<pre><span class="kw">pub fn</span> <span class="fn">build_strategy</span>(
    name: &amp;<span class="kw">str</span>,
    embedder: Option&lt;&amp;Arc&lt;dyn Embedder&gt;&gt;,
) -&gt; Result&lt;Box&lt;dyn RetrievalStrategy&gt;, StrategyError&gt; {
    <span class="kw">match</span> name {
        <span class="st">"bm25"</span> =&gt; Ok(Box::new(Bm25Strategy::new())),
        <span class="st">"vector"</span> =&gt; <span class="kw">match</span> embedder {
            Some(e) =&gt; Ok(Box::new(VectorStrategy::new(e.clone()))),
            None =&gt; Err(StrategyError::EmbedderRequired(name.to_string())),
        },
        other =&gt; Err(StrategyError::NotImplemented(other.to_string())),
    }
}</pre>
  </div>
</div>

<h2>build_embedder：只有 vector 才造，且只造一次</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">build_embedder</span> 在启动期跑一次。当且仅当 <span class="inline">strategy == "vector"</span> 时：</p>
  <ul>
    <li>读 <span class="inline">[retrieval.vector]</span> 段（缺段直接报错）。</li>
    <li>从 <span class="inline">api_key_env</span> 命名的环境变量取密钥——<strong>fail-fast</strong>，
      取不到立刻报错，且<strong>错误信息只提变量名、绝不打印密钥值</strong>。</li>
    <li>用配置构造 <span class="inline">OpenAiEmbedder</span>，再<strong>包一层 <span class="inline">CachingEmbedder</span></strong>
      （第 06 课的缓存层），最终返回 <span class="inline">Some(Arc&lt;dyn Embedder&gt;)</span>。</li>
    <li>其它策略一律返回 <span class="inline">None</span>。</li>
  </ul>
  <p>因为这个 <span class="inline">Arc</span> <strong>只构造一次</strong>并被 gateway 一直持有，所以<strong>缓存能跨 rebuild 持久存活</strong>——
  目录重建不会丢掉已经算好的嵌入。</p>
  <div class="codefile">
    <div class="cf-head"><span class="dot"></span><span class="path">crates/mcpgw/src/main.rs · build_embedder</span></div>
<pre><span class="kw">fn</span> <span class="fn">build_embedder</span>(cfg: &amp;config::Config)
  -&gt; Result&lt;Option&lt;Arc&lt;dyn retrieval::Embedder&gt;&gt;, String&gt; {
    <span class="kw">match</span> cfg.retrieval.strategy.as_str() {
        <span class="st">"vector"</span> =&gt; {
            <span class="kw">let</span> v = cfg.retrieval.vector.as_ref()
                .ok_or(<span class="st">"strategy=\"vector\" requires [retrieval.vector]"</span>)?;
            <span class="cm">// fail-fast：错误只提变量名，不含密钥值</span>
            <span class="kw">let</span> api_key = std::env::<span class="fn">var</span>(&amp;v.api_key_env)
                .<span class="fn">map_err</span>(|_| <span class="fn">format!</span>(<span class="st">"[retrieval.vector]: env {:?} is not set"</span>, v.api_key_env))?;
            <span class="kw">let</span> openai = embedder::OpenAiEmbedder::new(
                v.base_url.clone(), v.model.clone(), api_key,
                v.dim, v.timeout_ms.map(Duration::from_millis),
            );
            <span class="cm">// 只造一次 → 缓存跨 rebuild 持久</span>
            Ok(Some(Arc::new(retrieval::CachingEmbedder::new(Arc::new(openai)))))
        }
        _ =&gt; Ok(None),
    }
}</pre>
  </div>
  <p><span class="inline">prepare_state</span> 据此分支：拿到 <span class="inline">Some(embedder)</span> 走
  <span class="inline">GatewayState::with_embedder(&amp;cfg.retrieval.strategy, embedder)</span>；
  拿到 <span class="inline">None</span> 走 <span class="inline">GatewayState::new(&amp;cfg.retrieval.strategy)</span>。
  注意名字仍是原样传进去，由 <span class="inline">build_strategy</span> 再做一次校验。</p>
</div>

<h2>VectorConfig：把意图写进配置</h2>

<div class="card detail">
  <div class="tag">🔬 细节 / 代码对应</div>
  <p><span class="inline">[retrieval.vector]</span> 对应 <span class="inline">VectorConfig</span>（带 <span class="inline">deny_unknown_fields</span>，写错键名会被拒）。字段：</p>
  <table class="t">
    <tr><th>字段</th><th>类型</th><th>说明</th></tr>
    <tr><td class="mono">base_url</td><td class="mono">String</td><td>嵌入服务地址，默认 OpenAI <span class="inline">https://api.openai.com/v1</span></td></tr>
    <tr><td class="mono">model</td><td class="mono">String</td><td>嵌入模型名（必填）</td></tr>
    <tr><td class="mono">api_key_env</td><td class="mono">String</td><td>存放密钥的<strong>环境变量名</strong>（必填）——配置里只放变量名，不放密钥本身</td></tr>
    <tr><td class="mono">dim</td><td class="mono">Option&lt;usize&gt;</td><td>期望维度（可选，用于校验返回向量长度）</td></tr>
    <tr><td class="mono">timeout_ms</td><td class="mono">Option&lt;u64&gt;</td><td>单次请求超时（可选）</td></tr>
    <tr><td class="mono">batch_size</td><td class="mono">Option&lt;usize&gt;</td><td><strong>预留 / 未启用</strong>（见下方易错点）</td></tr>
  </table>
  <p>一份最小可用配置：</p>
<pre class="code"><span class="cm">[retrieval]</span>
strategy = <span class="st">"vector"</span>

<span class="cm">[retrieval.vector]</span>
base_url = <span class="st">"https://api.openai.com/v1"</span>
model = <span class="st">"text-embedding-3-small"</span>
api_key_env = <span class="st">"OPENAI_API_KEY"</span></pre>
</div>

<div class="card warn">
  <div class="tag">⚠️ 三个易错点</div>
  <ul>
    <li><strong><span class="inline">batch_size</span> 当前是预留 / 未启用字段</strong>：<span class="inline">OpenAiEmbedder</span>
      会把<strong>全部输入一次性发出</strong>，<strong>不做分块</strong>。别以为填了它就会自动批处理——它目前对行为<strong>毫无影响</strong>。</li>
    <li><strong><span class="inline">strategy = "vector"</span> 只在 <span class="inline">serve</span>（活网关）下生效</strong>：
      离线的 <span class="inline">search</span> / <span class="inline">get-details</span> CLI <strong>不会注入 embedder</strong>，
      因此那条路径下向量策略形同未配置。</li>
    <li><strong><span class="inline">validate()</span> 在 <span class="inline">strategy == "vector"</span> 时强制要求</strong>存在
      <span class="inline">[retrieval.vector]</span> 段（且 <span class="inline">base_url</span> / <span class="inline">model</span> / <span class="inline">api_key_env</span> 非空），缺段直接报配置错误。</li>
  </ul>
</div>

<div class="card key">
  <div class="tag">✅ 关键要点</div>
  <ul>
    <li><strong>fail-fast</strong>：缺凭证在<strong>启动期</strong>立刻报错，不拖到查询时。</li>
    <li><strong>缓存只建一次</strong>：<span class="inline">CachingEmbedder</span> 随 <span class="inline">Arc</span> 长存，跨 rebuild 不丢。</li>
    <li><strong>默认 bm25</strong>：向量是显式选择，未选时走零依赖的字面检索。</li>
    <li><strong>密钥只存 env 名</strong>：配置文件里只有 <span class="inline">api_key_env</span>，绝不落盘真实密钥。</li>
  </ul>
</div>

<div class="card spark">
  <div class="tag">💡 设计亮点</div>
  配置层把「<strong>要不要向量、向量打到哪、用哪个 key</strong>」声明化成一段 TOML；
  装配层（<span class="inline">build_embedder</span> → <span class="inline">prepare_state</span> → <span class="inline">build_strategy</span>）则在<strong>启动期一次性</strong>
  把这些声明兑现成可靠性约束——缺 key 立刻失败、缓存只建一次、名字与 embedder 的匹配性被强校验。意图与装配彻底分离。
</div>

<p>向量检索专章到此结束。下一步——<strong>Hybrid 检索与 RRF 融合</strong>——见第四部分的占位章节（待 M2-B 落地后写满）。</p>
"""
