"""Hover actions -- JS-tier and CDP-tier."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, load_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab

_JS_DIR = Path(__file__).parent.parent / "js"


class Hover(JsActionNode):
    """Dispatch ``mouseenter`` + ``mouseover`` on an element via JS."""

    js = load_js(_JS_DIR / "hover.js")

    def __init__(self, selector: str) -> None:
        self.selector = selector


class CdpHover(ActionNode):
    """Move the mouse to ``(x, y)`` via CDP ``Input.dispatchMouseEvent``."""

    def __init__(self, x: float, y: float) -> None:
        self.x = x
        self.y = y

    async def run(self, tab: Tab) -> None:
        await tab.dispatch_mouse_event("mouseMoved", self.x, self.y)

    def __repr__(self) -> str:
        return f"CdpHover(x={self.x}, y={self.y})"
