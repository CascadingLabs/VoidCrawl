"""Compose actions into sequential flows."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from void_crawl.actions._base import ActionNode
    from void_crawl.actions._protocol import Tab


@dataclass
class FlowResult:
    """Result of a flow execution."""

    results: list[object] = field(default_factory=list)

    @property
    def last(self) -> object:
        """The result of the final action, or ``None`` for empty flows."""
        return self.results[-1] if self.results else None


class Flow:
    """An ordered sequence of actions executed against a single tab.

    Example::

        flow = Flow(
            [
                ClickAt(100, 200),
                WaitForSelector("#menu", timeout=5.0),
                ClickElement("#menu-item-3"),
            ]
        )
        result = await flow.run(tab)
        print(result.last)

    Flows can also be built incrementally::

        flow = Flow()
        flow.add(ClickAt(100, 200))
        flow.add(WaitForSelector("#menu"))
        await flow.run(tab)
    """

    def __init__(self, actions: list[ActionNode] | None = None) -> None:
        self._actions: list[ActionNode] = list(actions or [])

    def add(self, action: ActionNode) -> Flow:
        """Append an action. Returns *self* for chaining."""
        self._actions.append(action)
        return self

    def __len__(self) -> int:
        return len(self._actions)

    async def run(self, tab: Tab) -> FlowResult:
        """Execute all actions sequentially against *tab*."""
        results: list[object] = []
        for action in self._actions:
            result = await action.run(tab)
            results.append(result)
        return FlowResult(results=results)

    def __repr__(self) -> str:
        actions_repr = ", ".join(repr(a) for a in self._actions)
        return f"Flow([{actions_repr}])"
