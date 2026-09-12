"""Focused black-box tests for the event-driven wait enforcement gate."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).parents[1] / "scripts" / "check-event-driven-waits.py"


@pytest.fixture
def source_tree(tmp_path: Path) -> Path:
    (tmp_path / "crates" / "core" / "src").mkdir(parents=True)
    (tmp_path / "voidcrawl").mkdir()
    return tmp_path


def run_gate(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--root", str(root)],
        check=False,
        capture_output=True,
        text=True,
    )


def test_detects_unapproved_unqualified_and_qualified_sleeps(source_tree: Path) -> None:
    (source_tree / "crates" / "core" / "src" / "lib.rs").write_text(
        "sleep(Duration::ZERO);\ntokio::time::sleep(Duration::ZERO).await;\n"
    )
    (source_tree / "voidcrawl" / "worker.py").write_text("await asyncio.sleep(0)\n")

    result = run_gate(source_tree)

    assert result.returncode == 1
    assert "crates/core/src/lib.rs:1" in result.stdout
    assert "crates/core/src/lib.rs:2" in result.stdout
    assert "voidcrawl/worker.py:1" in result.stdout
    assert "Remediation:" in result.stdout


def test_requires_immediate_comment_block_with_substantive_rationale(
    source_tree: Path,
) -> None:
    source = source_tree / "voidcrawl" / "worker.py"
    source.write_text(
        "# EVENT_DRIVEN_SLEEP_APPROVED: too short\n"
        "await time.sleep(1)\n"
        "# EVENT_DRIVEN_SLEEP_APPROVED:\n"
        "# external hardware protocol requires a bounded delay\n"
        "# after a confirmed event\n"
        "await asyncio.sleep(1)\n"
    )

    result = run_gate(source_tree)

    assert result.returncode == 1
    assert "voidcrawl/worker.py:2" in result.stdout
    assert "voidcrawl/worker.py:5" not in result.stdout


def test_excludes_tests_benchmarks_generated_and_vendor(source_tree: Path) -> None:
    for relative in (
        "crates/core/src/vendor/dependency.rs",
        "crates/core/src/benchmarks/latency.rs",
        "voidcrawl/tests/test_wait.py",
        "voidcrawl/generated/client.py",
    ):
        path = source_tree / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("sleep(1)\n")

    result = run_gate(source_tree)

    assert result.returncode == 0, result.stdout


def test_allows_timeout_and_sleep_until(source_tree: Path) -> None:
    (source_tree / "crates" / "core" / "src" / "lib.rs").write_text(
        "let deadline = timeout(duration, event.wait());\n"
        "clock.sleep_until(deadline);\n"
        "timeout_at(deadline, receiver.recv());\n"
    )

    result = run_gate(source_tree)

    assert result.returncode == 0, result.stdout
