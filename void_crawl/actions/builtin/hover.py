"""Hover / mouse-move actions — both JS-tier and CDP-tier."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, load_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab

_JS_DIR = Path(__file__).parent.parent / "js"


class Hover(JsActionNode):
    """Dispatch ``mouseenter`` + ``mouseover`` on an element via JS.

    Triggers CSS ``:hover`` styles and JS hover handlers without
    moving the CDP-level cursor.

    Args:
        selector: CSS selector targeting the element to hover.
    """

    js = load_js(_JS_DIR / "hover.js")

    def __init__(self, selector: str) -> None:
        self.selector = selector


class CdpHover(ActionNode):
    """Move the virtual mouse cursor to ``(x, y)`` via CDP.

    Sends a ``mouseMoved`` event.  Unlike :class:`Hover`, this moves
    the actual CDP cursor position, which is needed for subsequent
    :class:`CdpClick` calls to land correctly.

    Args:
        x: Horizontal page coordinate.
        y: Vertical page coordinate.
    """

    def __init__(self, x: float, y: float) -> None:
        self.x = x
        self.y = y

    async def run(self, tab: Tab) -> None:
        """Dispatch a ``mouseMoved`` event to ``(x, y)``.

        Args:
            tab: Tab-like object to send the hover event to.
        """
        await tab.dispatch_mouse_event("mouseMoved", self.x, self.y)

    def __repr__(self) -> str:
        return f"CdpHover(x={self.x}, y={self.y})"
