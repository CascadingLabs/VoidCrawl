"""Type stubs for voidcrawl.actions._protocol."""

from __future__ import annotations

from typing import Protocol, runtime_checkable

__all__ = ["JsTab", "Tab"]

@runtime_checkable
class JsTab(Protocol):
    """Minimal protocol for JavaScript-only actions.

    Any object with an async ``evaluate_js`` method satisfies this protocol.
    """

    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* in the page and return the result."""
        ...
    async def eval_js(self, expression: str) -> object:
        """Alias for :meth:`evaluate_js` — short form used by MCP tooling."""
        ...

@runtime_checkable
class Tab(JsTab, Protocol):
    """Full protocol covering JS evaluation and CDP input commands.

    Both :class:`~voidcrawl.Page` and :class:`~voidcrawl.PooledTab` satisfy
    this protocol.
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
        """Send a low-level CDP ``Input.dispatchMouseEvent``."""
        ...

    async def dispatch_key_event(
        self,
        event_type: str,
        key: str | None = None,
        code: str | None = None,
        text: str | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchKeyEvent``."""
        ...
