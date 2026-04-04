"""Wait / timing actions (JS-tier).

Provides :class:`WaitForSelector` (poll for an element) and
:class:`WaitForTimeout` (fixed delay inside the page context).
"""

from __future__ import annotations

from pathlib import Path

from void_crawl.actions._base import JsActionNode, inline_js, load_js

_JS_DIR = Path(__file__).parent.parent / "js"


class WaitForSelector(JsActionNode):
    """Poll until a CSS selector matches an element, with timeout.

    Polls at a short interval inside the browser context.  Resolves as
    soon as ``document.querySelector(selector)`` is non-null, or throws
    a JS ``Error`` when *timeout* seconds elapse.

    Args:
        selector: CSS selector to wait for.
        timeout: Maximum wait time in seconds. Defaults to ``10.0``.
    """

    js = load_js(_JS_DIR / "wait_for_selector.js")

    def __init__(self, selector: str, timeout: float = 10.0) -> None:
        self.selector = selector
        self.timeout = timeout


class WaitForTimeout(JsActionNode):
    """Sleep for *ms* milliseconds **inside the browser context**.

    This pauses the JS execution inside the page, not the Python event
    loop.  Useful for waiting on animations or debounced handlers.

    Args:
        ms: Delay in milliseconds.
    """

    js = inline_js("return new Promise(r => setTimeout(r, __params.ms));")

    def __init__(self, ms: int) -> None:
        self.ms = ms
