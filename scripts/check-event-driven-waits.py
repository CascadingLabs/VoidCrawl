#!/usr/bin/env python3
"""Reject production sleeps unless a reviewed event-driven exception is documented."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

APPROVAL_MARKER = "EVENT_DRIVEN_SLEEP_APPROVED:"
MIN_RATIONALE_NON_WHITESPACE = 40
EXCLUDED_PARTS = frozenset(
    {
        "test",
        "tests",
        "bench",
        "benches",
        "benchmark",
        "benchmarks",
        "generated",
        "vendor",
        "target",
        "__pycache__",
    }
)
# Matches sleep(...) and qualified forms such as tokio::time::sleep(...),
# time.sleep(...), and asyncio.sleep(...), but deliberately not sleep_until(...).
SLEEP_CALL = re.compile(
    r"(?<![A-Za-z0-9_])(?:[A-Za-z_]\w*(?:(?:::|\.)[A-Za-z_]\w*)*(?:::|\.))?sleep\s*\("
)


def production_sources(root: Path) -> list[Path]:
    """Return only non-generated production Rust and Python source files."""
    sources: list[Path] = []
    for src_dir in root.glob("crates/*/src"):
        if src_dir.is_dir():
            sources.extend(
                path
                for path in src_dir.rglob("*.rs")
                if not (set(path.relative_to(root).parts) & EXCLUDED_PARTS)
            )
    python_dir = root / "voidcrawl"
    if python_dir.is_dir():
        sources.extend(
            path
            for path in python_dir.rglob("*.py")
            if not (set(path.relative_to(root).parts) & EXCLUDED_PARTS)
        )
    return sorted(sources)


def has_valid_approval(lines: list[str], sleep_line_index: int) -> bool:
    """Check the immediately preceding comment block for a reviewed approval."""
    preceding: list[str] = []
    index = sleep_line_index - 1
    while index >= 0 and not lines[index].strip():
        index -= 1
    while index >= 0:
        line = lines[index]
        stripped = line.lstrip()
        if not stripped.startswith(("//", "#")):
            break
        preceding.append(stripped[2:] if stripped.startswith("//") else stripped[1:])
        index -= 1

    approval = "\n".join(reversed(preceding))
    marker_index = approval.find(APPROVAL_MARKER)
    if marker_index < 0:
        return False
    rationale = (
        approval[:marker_index] + approval[marker_index + len(APPROVAL_MARKER) :]
    )
    return len(re.sub(r"\s+", "", rationale)) >= MIN_RATIONALE_NON_WHITESPACE


def violations(root: Path) -> list[tuple[Path, int]]:
    """Find unapproved sleep calls in the production source tree."""
    found: list[tuple[Path, int]] = []
    for path in production_sources(root):
        lines = path.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            if SLEEP_CALL.search(line) and not has_valid_approval(lines, index):
                found.append((path.relative_to(root), index + 1))
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=Path.cwd(), help="repository root to scan"
    )
    args = parser.parse_args()
    root = args.root.resolve()
    found = violations(root)
    if not found:
        return 0

    for path, line in found:
        print(f"{path}:{line}: sleep-based synchronization is forbidden")
    print(
        "Remediation: replace the sleep with an event/channel/CDP wait or, "
        "for a rare reviewed exception, put EVENT_DRIVEN_SLEEP_APPROVED: "
        "followed by at least 40 non-whitespace rationale characters in the "
        "immediately preceding contiguous comment block."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
