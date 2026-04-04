"""Scroll actions — JS-tier and CDP-tier.

JS-tier: :class:`ScrollTo` (absolute) and :class:`ScrollBy` (relative).
CDP-tier: :class:`CdpScroll` (generic wheel event) plus convenience
wrappers :class:`CdpScrollDown`, :class:`CdpScrollUp`,
:class:`CdpScrollLeft`, and :class:`CdpScrollRight`.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, inline_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab


class ScrollTo(JsActionNode):
    """Scroll the window to an absolute position via ``window.scrollTo``.

    Args:
        x: Horizontal scroll offset in pixels. Defaults to ``0``.
        y: Vertical scroll offset in pixels. Defaults to ``0``.
    """

    js = inline_js("window.scrollTo(__params.x, __params.y); return null;")

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


class ScrollBy(JsActionNode):
    """Scroll the window by a relative offset via ``window.scrollBy``.

    Args:
        dx: Horizontal delta in pixels (positive = right). Defaults to ``0``.
        dy: Vertical delta in pixels (positive = down). Defaults to ``0``.
    """

    js = inline_js("window.scrollBy(__params.dx, __params.dy); return null;")

    def __init__(self, dx: int = 0, dy: int = 0) -> None:
        self.dx = dx
        self.dy = dy


class CdpScroll(ActionNode):
    """Scroll via a CDP ``mouseWheel`` event fired at ``(x, y)``.

    For most use cases prefer the convenience subclasses
    :class:`CdpScrollDown`, :class:`CdpScrollUp`,
    :class:`CdpScrollLeft`, and :class:`CdpScrollRight`.

    Args:
        x: Horizontal page coordinate for the wheel event origin.
        y: Vertical page coordinate for the wheel event origin.
        delta_x: Horizontal scroll amount (positive = right).
        delta_y: Vertical scroll amount (positive = down).
    """

    def __init__(
        self, x: float = 0, y: float = 0, delta_x: float = 0, delta_y: float = 0
    ) -> None:
        self.x = x
        self.y = y
        self.delta_x = delta_x
        self.delta_y = delta_y

    async def run(self, tab: Tab) -> None:
        """Dispatch a ``mouseWheel`` event at ``(x, y)``.

        Args:
            tab: Tab-like object to send the scroll event to.
        """
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
    """Scroll **down** by *pixels* at ``(x, y)`` via CDP.

    Args:
        pixels: Distance to scroll in pixels. Defaults to ``100``.
        x: Horizontal origin for the wheel event.
        y: Vertical origin for the wheel event.
    """

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_y=pixels)


class CdpScrollUp(CdpScroll):
    """Scroll **up** by *pixels* at ``(x, y)`` via CDP.

    Args:
        pixels: Distance to scroll in pixels. Defaults to ``100``.
        x: Horizontal origin for the wheel event.
        y: Vertical origin for the wheel event.
    """

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_y=-pixels)


class CdpScrollRight(CdpScroll):
    """Scroll **right** by *pixels* at ``(x, y)`` via CDP.

    Args:
        pixels: Distance to scroll in pixels. Defaults to ``100``.
        x: Horizontal origin for the wheel event.
        y: Vertical origin for the wheel event.
    """

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_x=pixels)


class CdpScrollLeft(CdpScroll):
    """Scroll **left** by *pixels* at ``(x, y)`` via CDP.

    Args:
        pixels: Distance to scroll in pixels. Defaults to ``100``.
        x: Horizontal origin for the wheel event.
        y: Vertical origin for the wheel event.
    """

    def __init__(self, pixels: float = 100, x: float = 0, y: float = 0) -> None:
        super().__init__(x=x, y=y, delta_x=-pixels)
