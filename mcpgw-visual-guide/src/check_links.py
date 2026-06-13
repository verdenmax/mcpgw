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
