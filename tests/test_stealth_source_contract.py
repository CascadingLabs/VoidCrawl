"""Source-level regression tests for the low-CDP stealth contract."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_minimal_network_init_preserves_bad_tls_without_network_enable() -> None:
    source = (ROOT / "vendor/chromiumoxide/src/handler/network.rs").read_text()
    match = re.search(
        r"if cdp_mode\.is_minimal\(\) \{(?P<body>.*?)return CommandChain::new",
        source,
        re.DOTALL,
    )
    assert match is not None
    body = match.group("body")
    assert "SetIgnoreCertificateErrorsParams::new(true)" in body
    assert "EnableParams::default()" not in body


def test_frame_eval_lazily_enables_runtime_and_reports_state() -> None:
    source = (ROOT / "crates/core/src/page.rs").read_text()
    assert "runtime_enabled:        AtomicBool" in source
    assert ".enable_runtime()" in source
    assert "self.runtime_enabled.store(true, Ordering::Relaxed)" in source
    assert "frame_execution_context_with_runtime(frame_id, frame_url_pattern)" in source
    assert "low_cdp: !(network_enabled || runtime_enabled)" in source


def test_docs_do_not_link_to_deleted_experiment_docs() -> None:
    stale_links: list[str] = []
    for path in (ROOT / "docs").glob("**/*.md"):
        text = path.read_text()
        for match in re.finditer(r"\[[^\]]+\]\(([^)#]+\.md)\)", text):
            target = match.group(1)
            if "://" in target:
                continue
            candidate = (path.parent / target).resolve()
            if not candidate.exists():
                stale_links.append(f"{path.relative_to(ROOT)} -> {target}")
    assert stale_links == []
