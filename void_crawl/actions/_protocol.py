"""Structural protocols unifying :class:`Page` and :class:`PooledTab` for actions.

Actions accept any object satisfying :class:`Tab` (full protocol) or
:class:`JsTab` (JS-only subset), so they work interchangeably with both
single-page sessions and pooled tabs.
"""

from __future__ import annotations

from typing import Protocol, runtime_checkable


@runtime_checkable
class JsTab(Protocol):
    """Minimal protocol for JavaScript-only actions.

    Any object with an async ``evaluate_js`` method satisfies this
    protocol — including :class:`Page`, :class:`PooledTab`, and test
    mocks.  Used by :class:`~void_crawl.actions.JsActionNode`.
    """

    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* in the page and return the result.

        Args:
            expression: JavaScript expression or IIFE string.

        Returns:
            The JSON-deserialised return value from the browser.
        """
        ...


@runtime_checkable
class Tab(JsTab, Protocol):
    """Full protocol covering JS evaluation **and** CDP input commands.

    Both :class:`Page` and :class:`PooledTab` satisfy this protocol.
    CDP-level actions (e.g. :class:`~void_crawl.actions.CdpClick`) require
    this protocol rather than the simpler :class:`JsTab`.
    """

    async def dispatch_mouse_event(
        self,
        event_type: str,
        x: float,
        y: float,
        button: str = "left",
        click_count: int = 1,
        delta_x: float | None = None,
        delta_y: float | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchMouseEvent``.

        Args:
            event_type: One of ``"mousePressed"``, ``"mouseReleased"``,
                ``"mouseMoved"``, or ``"mouseWheel"``.
            x: Horizontal page coordinate.
            y: Vertical page coordinate.
            button: Mouse button — ``"left"``, ``"right"``, or ``"middle"``.
            click_count: Number of clicks (usually ``1``).
            delta_x: Horizontal scroll delta (``mouseWheel`` only).
            delta_y: Vertical scroll delta (``mouseWheel`` only).
            modifiers: Bit field for modifier keys (Ctrl=1, Shift=2, etc.).
        """
        ...

    async def dispatch_key_event(
        self,
        event_type: str,
        key: str | None = None,
        code: str | None = None,
        text: str | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchKeyEvent``.

        Args:
            event_type: ``"keyDown"``, ``"keyUp"``, ``"rawKeyDown"``, or
                ``"char"``.
            key: DOM ``KeyboardEvent.key`` value (e.g. ``"Enter"``).
            code: Physical key code (e.g. ``"KeyA"``).
            text: Character to insert (e.g. ``"a"``).
            modifiers: Bit field for modifier keys.
        """
        ...
