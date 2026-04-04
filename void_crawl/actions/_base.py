"""Base action node abstraction.

Provides :class:`ActionNode` (the abstract base for all browser actions),
:class:`JsActionNode` (the JavaScript-backed variant), and the
:class:`JsSource` / :func:`load_js` / :func:`inline_js` helpers for
packaging JS snippets.
"""

from __future__ import annotations

import inspect
import json
from abc import ABC, abstractmethod
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from void_crawl.actions._protocol import JsTab, Tab


class JsSource:
    """Immutable wrapper around a JavaScript snippet string.

    Created via :func:`load_js` (file-based) or :func:`inline_js`
    (string literal).  Used as the ``js`` class attribute on
    :class:`JsActionNode` subclasses.

    Args:
        js: Raw JavaScript source code.
    """

    __slots__ = ("_js",)

    def __init__(self, js: str) -> None:
        self._js = js

    @property
    def js(self) -> str:
        """The raw JavaScript source string."""
        return self._js

    def __repr__(self) -> str:
        preview = self._js[:60].replace("\n", " ")
        return (
            f"JsSource({preview!r}...)"
            if len(self._js) > 60
            else f"JsSource({preview!r})"
        )


def load_js(path: str | Path) -> JsSource:
    """Load JavaScript from a ``.js`` file on disk.

    Absolute paths are used as-is.  Relative paths are resolved from the
    **caller's** source file, so ``load_js("click_at.js")`` works when
    the ``.js`` lives next to the calling ``.py``.

    Args:
        path: Filesystem path to the ``.js`` file.

    Returns:
        The loaded JavaScript source.
    """
    p = Path(path)
    if not p.is_absolute():
        caller_file = inspect.stack()[1].filename
        p = Path(caller_file).parent / p
    return JsSource(p.read_text(encoding="utf-8"))


def inline_js(code: str) -> JsSource:
    """Create a :class:`JsSource` from an inline string literal.

    Args:
        code: Raw JavaScript source code.

    Returns:
        The wrapped JavaScript source.
    """
    return JsSource(code)


def _build_expression(js_source: JsSource, params: dict[str, Any]) -> str:
    """Build a full JS expression with a ``__params`` preamble.

    Wraps in an async IIFE so ``const`` declarations don't leak and
    ``await`` can be used inside the snippet.
    """
    params_json = json.dumps(params, default=str)
    return f"(async () => {{ const __params = {params_json}; {js_source.js} }})()"


class ActionNode(ABC):
    """Abstract base for all browser actions.

    Subclass and implement :meth:`run` to create a custom action.  Use
    :class:`JsActionNode` when the action can be expressed as a single
    JavaScript snippet; subclass ``ActionNode`` directly for CDP-level
    actions that need :meth:`~void_crawl.actions.Tab.dispatch_mouse_event`
    or :meth:`~void_crawl.actions.Tab.dispatch_key_event`.
    """

    @abstractmethod
    async def run(self, tab: Tab) -> object:
        """Execute this action against *tab*.

        Args:
            tab: Any object satisfying the :class:`~void_crawl.actions.Tab`
                protocol (e.g. :class:`Page` or :class:`PooledTab`).

        Returns:
            The action result — type varies by action.
        """
        ...

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"


class JsActionNode(ActionNode):
    """Action executed by evaluating a JavaScript snippet in the page.

    Subclasses set the ``js`` class attribute (via :func:`load_js` or
    :func:`inline_js`) and store their parameters as instance attributes
    in ``__init__``.  At execution time, all instance attributes are
    serialised to JSON and injected as the ``__params`` object visible
    inside the snippet.

    By default :meth:`params` returns ``vars(self)``; override it only
    if you need to transform or filter attributes.

    Example:
        >>> class ClickAt(JsActionNode):
        ...     js = inline_js('''
        ...         const el = document.elementFromPoint(__params.x, __params.y);
        ...         if (el) el.click();
        ...         return el ? el.tagName : null;
        ...     ''')
        ...
        ...     def __init__(self, x: int, y: int) -> None:
        ...         self.x = x
        ...         self.y = y
    """

    js: JsSource

    def params(self) -> dict[str, Any]:
        """Return the parameters injected as ``__params`` in the JS snippet.

        Defaults to ``vars(self)``.  Override to transform, rename, or
        filter attributes before they reach JavaScript.

        Returns:
            A JSON-serialisable dict of parameter names to values.
        """
        return vars(self)

    async def run(self, tab: JsTab) -> object:
        """Evaluate the JS snippet in *tab* with the current :meth:`params`.

        Args:
            tab: Any object satisfying :class:`~void_crawl.actions.JsTab`.

        Returns:
            The JSON-deserialised return value from the snippet.
        """
        expression = _build_expression(self.js, self.params())
        return await tab.evaluate_js(expression)

    def __repr__(self) -> str:
        cls = type(self).__name__
        p = self.params()
        args = ", ".join(f"{k}={v!r}" for k, v in p.items())
        return f"{cls}({args})"


# CdpActionNode was removed — CDP actions subclass ActionNode directly.
