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
