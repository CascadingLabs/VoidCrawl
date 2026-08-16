"""Relative Markdown links in docs/ must resolve to files that exist.

Docs get moved and deleted far more often than the links pointing at them get
updated, and a dead relative link is invisible until a reader hits it. This walks
every ``docs/**/*.md`` and resolves each relative ``.md`` target.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

#: Markdown inline links, capturing the target and ignoring any ``#anchor``.
MD_LINK = re.compile(r"\[[^\]]+\]\(([^)#]+\.md)\)")


def test_docs_have_no_dead_relative_markdown_links() -> None:
    stale: list[str] = []
    for path in sorted((ROOT / "docs").glob("**/*.md")):
        for match in MD_LINK.finditer(path.read_text()):
            target = match.group(1)
            if "://" in target:
                continue
            if not (path.parent / target).resolve().exists():
                stale.append(f"{path.relative_to(ROOT)} -> {target}")

    assert stale == [], "dead relative links in docs/:\n  " + "\n  ".join(stale)
