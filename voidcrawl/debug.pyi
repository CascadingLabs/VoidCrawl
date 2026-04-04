"""Type stubs for voidcrawl.debug."""

from __future__ import annotations

from typing import TypeVar

from voidcrawl.actions._base import ActionNode
from voidcrawl.actions._flow import Flow, FlowResult
from voidcrawl.actions._protocol import Tab

__all__ = ["DebugSession", "vc_breakpoint"]

_T = TypeVar("_T", bound=type)

def vc_breakpoint(cls: _T) -> _T:
    """Mark an action class as a debugger breakpoint.

    When a :class:`DebugSession` encounters an action whose class is marked
    with this decorator, it pauses execution regardless of stepping mode.
    """
    ...

class DebugSession:
    """Interactive step debugger for browser actions."""

    def __init__(
        self,
        tab: Tab,
        *,
        start_url: str | None = None,
        stepping: bool = True,
        step_delay: float = 0.3,
        highlight: bool = True,
    ) -> None: ...
    def add(self, action: ActionNode) -> DebugSession:
        """Append a single action to the execution queue."""
        ...

    def add_flow(self, flow: Flow) -> DebugSession:
        """Append every action from a :class:`Flow` to the queue."""
        ...

    async def start(self) -> FlowResult:
        """Run the queued actions with interactive debug control."""
        ...
