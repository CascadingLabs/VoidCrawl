"""Scroll actions -- JS-tier and CDP-tier."""

from __future__ import annotations

from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, inline_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab


class ScrollTo(JsActionNode):
    """Scroll to an absolute position."""

    js = inline_js("window.scrollTo(__params.x, __params.y); return null;")

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


class ScrollBy(JsActionNode):
    """Scroll by a relative offset."""

    js = inline_js("window.scrollBy(__params.dx, __params.dy); return null;")

    def __init__(self, dx: int = 0, dy: int = 0) -> None:
        self.dx = dx
        self.dy = dy


class CdpScroll(ActionNode):
    """Scroll via CDP ``mouseWheel`` event at ``(x, y)``."""

    def __init__(
        self, x: float = 0, y: float = 0, delta_x: float = 0, delta_y: float = 0
    ) -> None:
        self.x = x
        self.y = y
        self.delta_x = delta_x
        self.delta_y = delta_y

    async def run(self, tab: Tab) -> None:
        await tab.dispatch_mouse_event(
            "mouseWheel",
            self.x,
            self.y,
            delta_x=self.delta_x,
            delta_y=self.delta_y,
        )

    def __repr__(self) -> str:
        return (
            f"CdpScroll(x={self.x}, y={self.y}, "
            f"delta_x={self.delta_x}, delta_y={self.delta_y})"
        )


class CdpScrollDown(CdpScroll):
    """Scroll down by *pixels* at ``(x, y)`` via CDP."""

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_y=pixels)


class CdpScrollUp(CdpScroll):
    """Scroll up by *pixels* at ``(x, y)`` via CDP."""

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_y=-pixels)


class CdpScrollRight(CdpScroll):
    """Scroll right by *pixels* at ``(x, y)`` via CDP."""

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_x=pixels)


class CdpScrollLeft(CdpScroll):
    """Scroll left by *pixels* at ``(x, y)`` via CDP."""

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_x=-pixels)
