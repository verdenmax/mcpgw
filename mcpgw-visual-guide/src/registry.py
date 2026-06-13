"""Single source of truth: ordered map of output filename -> lesson HTML content.

build.py imports this so the lesson set stays in sync with shell.PAGES.
"""
import shell
import part1_macro as p1
import part2_vector as p2
import part3_internals as p3
import part4_next as p4

# 01-08 (written-through lessons) are filled by later tasks; until then they use
# a one-line stub. 09-15 are navigable 'under construction' placeholder pages.
_STUB = {fname: f"<p>（待填充：{title}）</p>" for fname, title, _part in shell.PAGES}

CONTENT = {
    **_STUB,
    "01-what-is-mcpgw.html": p1.LESSON_01,
    "02-architecture.html": p1.LESSON_02,
    "03-call-lifecycle.html": p1.LESSON_03,
    "04-vector-overview.html": p2.LESSON_04,
    "05-embedder.html": p2.LESSON_05,
    "06-caching-embedder.html": p2.LESSON_06,
    "07-vector-strategy.html": p2.LESSON_07,
    "08-wiring-config.html": p2.LESSON_08,
    "09-catalog.html": p3.LESSON_09,
    "10-upstream.html": p3.LESSON_10,
    "11-gateway-metatools.html": p3.LESSON_11,
    "12-downstream.html": p3.LESSON_12,
    "13-retrieval-bm25.html": p3.LESSON_13,
    "14-config.html": p3.LESSON_14,
    "15-hybrid-rrf.html": p4.LESSON_15,
}
