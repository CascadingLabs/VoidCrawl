"""Compose multiple actions into sequential flows.

A :class:`Flow` groups ordered :class:`~void_crawl.actions.ActionNode`
instances and runs them one-by-one against a single tab, collecting
every result into a :class:`FlowResult`.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from void_crawl.actions._base import ActionNode
    from void_crawl.actions._protocol import Tab


@dataclass
class FlowResult:
    """Aggregated result of a :class:`Flow` execution.

    Attributes:
        results: Ordered list of return values, one per action.
        last: The return value of the final action, or ``None`` for
            empty flows (read-only property).
    """

    results: list[object] = field(default_factory=list)

    @property
    def last(self) -> object:
        """The return value of the final action, or ``None`` for empty flows."""
        return self.results[-1] if self.results else None


class Flow:
    """An ordered sequence of actions executed against a single tab.

    Actions run sequentially in the order added; each result is captured
    in the returned :class:`FlowResult`.

    Args:
        actions: Initial list of actions.  May be ``None`` or omitted to
            start with an empty flow and use :meth:`add`.

    Example:
        Build up-front::

            flow = Flow(
                [
                    ClickAt(100, 200),
                    WaitForSelector("#menu", timeout=5.0),
                    ClickElement("#menu-item-3"),
                ]
            )
            result = await flow.run(tab)
            print(result.last)

        Build incrementally::

            flow = Flow()
            flow.add(ClickAt(100, 200))
            flow.add(WaitForSelector("#menu"))
            await flow.run(tab)
    """

    def __init__(self, actions: list[ActionNode] | None = None) -> None:
        self._actions: list[ActionNode] = list(actions or [])

    def add(self, action: ActionNode) -> Flow:
        """Append an action and return *self* for chaining.

        Args:
            action: The action to append.

        Returns:
            This instance, for builder-style chaining.
        """
        self._actions.append(action)
        return self

    def __len__(self) -> int:
        return len(self._actions)

    async def run(self, tab: Tab) -> FlowResult:
        """Execute all actions sequentially against *tab*.

        Args:
            tab: Any object satisfying the ``Tab`` protocol.

        Returns:
            One result per action.
        """
        results: list[object] = []
        for action in self._actions:
            result = await action.run(tab)
            results.append(result)
        return FlowResult(results=results)

    def __repr__(self) -> str:
        actions_repr = ", ".join(repr(a) for a in self._actions)
        return f"Flow([{actions_repr}])"
