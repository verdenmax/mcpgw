"""Single source of truth: ordered map of output filename -> lesson HTML content.

build.py imports this so the lesson set stays in sync with shell.PAGES.
"""
import shell

# Filename -> lesson HTML. Real content is filled in by later tasks; until then
# every page renders a one-line stub so the site builds end-to-end.
CONTENT = {fname: f"<p>（待填充：{title}）</p>" for fname, title, _part in shell.PAGES}
