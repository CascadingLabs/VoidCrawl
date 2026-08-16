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


def test_attached_pages_refresh_targets_before_listing() -> None:
    source = (ROOT / "crates/core/src/session.rs").read_text()
    pages_body = source.split("pub async fn pages(&self)", maxsplit=1)[1].split(
        "pub async fn websocket_url",
        maxsplit=1,
    )[0]
    assert "if self.attached" in pages_body
    assert "browser.fetch_targets().await" in pages_body


def test_benchmark_wraps_voidcrawl_startup_in_timeout() -> None:
    source = (ROOT / "scripts/bench_antibot_cdp.py").read_text()
    run_body = source.split("async def run_voidcrawl", maxsplit=1)[1].split(
        "async def stop_browser",
        maxsplit=1,
    )[0]
    assert "async def run_once()" in run_body
    assert "asyncio.wait_for(run_once()" in run_body


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
