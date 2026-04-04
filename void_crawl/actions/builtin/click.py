"""Click actions — both JS-tier and CDP-tier.

Provides coordinate-based clicks (:class:`ClickAt`, :class:`CdpClick`),
selector-based clicks (:class:`ClickElement`), and a long-press action
(:class:`CdpClickAndHold`).
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, inline_js, load_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab

_JS_DIR = Path(__file__).parent.parent / "js"


class ClickAt(JsActionNode):
    """Click the element at page coordinates ``(x, y)`` via JS events.

    Uses ``document.elementFromPoint`` to resolve the target and
    dispatches synthetic mouse events.

    Args:
        x: Horizontal page coordinate (pixels from left).
        y: Vertical page coordinate (pixels from top).
    """

    js = load_js(_JS_DIR / "click_at.js")

    def __init__(self, x: int, y: int) -> None:
        self.x = x
        self.y = y


class ClickElement(JsActionNode):
    """Click the first element matching a CSS *selector* via JS.

    Raises a JS ``Error`` if no element matches.

    Args:
        selector: CSS selector string (e.g. ``"#submit-btn"``).
    """

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
el.click();
return null;
""")

    def __init__(self, selector: str) -> None:
        self.selector = selector


class CdpClick(ActionNode):
    """Click at ``(x, y)`` via CDP ``Input.dispatchMouseEvent``.

    Sends a ``mousePressed`` followed by ``mouseReleased``.  More
    realistic than JS-level clicks — useful for pages that inspect
    event coordinates.

    Args:
        x: Horizontal page coordinate.
        y: Vertical page coordinate.
        button: Mouse button — ``"left"``, ``"right"``, or ``"middle"``.
    """

    def __init__(self, x: float, y: float, button: str = "left") -> None:
        self.x = x
        self.y = y
        self.button = button

    async def run(self, tab: Tab) -> None:
        """Dispatch ``mousePressed`` then ``mouseReleased`` at ``(x, y)``.

        Args:
            tab: Tab-like object to send the click events to.
        """
        await tab.dispatch_mouse_event(
            "mousePressed", self.x, self.y, button=self.button, click_count=1
        )
        await tab.dispatch_mouse_event(
            "mouseReleased", self.x, self.y, button=self.button, click_count=1
        )

    def __repr__(self) -> str:
        return f"CdpClick(x={self.x}, y={self.y}, button={self.button!r})"


class CdpClickAndHold(ActionNode):
    """Mouse-down, hold for *duration_ms*, then mouse-up via CDP.

    Useful for triggering long-press menus or drag initialisation.

    Args:
        x: Horizontal page coordinate.
        y: Vertical page coordinate.
        duration_ms: How long to hold the button, in milliseconds.
        button: Mouse button — ``"left"``, ``"right"``, or ``"middle"``.
    """

    def __init__(
        self, x: float, y: float, duration_ms: int = 500, button: str = "left"
    ) -> None:
        self.x = x
        self.y = y
        self.duration_ms = duration_ms
        self.button = button

    async def run(self, tab: Tab) -> None:
        """Press, hold for *duration_ms*, then release at ``(x, y)``.

        Args:
            tab: Tab-like object to send the mouse events to.
        """
        await tab.dispatch_mouse_event(
            "mousePressed", self.x, self.y, button=self.button, click_count=1
        )
        await asyncio.sleep(self.duration_ms / 1000.0)
        await tab.dispatch_mouse_event(
            "mouseReleased", self.x, self.y, button=self.button, click_count=1
        )

    def __repr__(self) -> str:
        return (
            f"CdpClickAndHold(x={self.x}, y={self.y}, "
            f"duration_ms={self.duration_ms}, button={self.button!r})"
        )
