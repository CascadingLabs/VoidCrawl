"""Wait actions (JS-tier)."""

from __future__ import annotations

from pathlib import Path

from void_crawl.actions._base import JsActionNode, inline_js, load_js

_JS_DIR = Path(__file__).parent.parent / "js"


class WaitForSelector(JsActionNode):
    """Poll until a CSS selector matches, with timeout."""

    js = load_js(_JS_DIR / "wait_for_selector.js")

    def __init__(self, selector: str, timeout: float = 10.0) -> None:
        self.selector = selector
        self.timeout = timeout


class WaitForTimeout(JsActionNode):
    """Sleep for *ms* milliseconds in the browser context."""

    js = inline_js("return new Promise(r => setTimeout(r, __params.ms));")

    def __init__(self, ms: int) -> None:
        self.ms = ms
