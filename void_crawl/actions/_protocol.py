"""Protocols unifying Page and PooledTab for action execution."""

from __future__ import annotations

from typing import Protocol, runtime_checkable


@runtime_checkable
class JsTab(Protocol):
    """Minimal protocol: only needs ``evaluate_js``.

    This is what :class:`JsActionNode` requires — any object with an
    ``evaluate_js`` method will work, even a mock.
    """

    async def evaluate_js(self, expression: str) -> object: ...


@runtime_checkable
class Tab(JsTab, Protocol):
    """Full protocol covering both JS eval and CDP input commands.

    Both ``Page`` and ``PooledTab`` satisfy this protocol.
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
    ) -> None: ...

    async def dispatch_key_event(
        self,
        event_type: str,
        key: str | None = None,
        code: str | None = None,
        text: str | None = None,
        modifiers: int | None = None,
    ) -> None: ...
