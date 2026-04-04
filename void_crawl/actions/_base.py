"""Base action node abstraction."""

from __future__ import annotations

import inspect
import json
from abc import ABC, abstractmethod
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from void_crawl.actions._protocol import JsTab, Tab


class JsSource:
    """Holds a JS snippet -- loaded from a file or defined inline."""

    __slots__ = ("_js",)

    def __init__(self, js: str) -> None:
        self._js = js

    @property
    def js(self) -> str:
        return self._js

    def __repr__(self) -> str:
        preview = self._js[:60].replace("\n", " ")
        return (
            f"JsSource({preview!r}...)"
            if len(self._js) > 60
            else f"JsSource({preview!r})"
        )


def load_js(path: str | Path) -> JsSource:
    """Load JavaScript from a ``.js`` file.

    Absolute paths are used as-is. Relative paths are resolved from the
    caller's source file.
    """
    p = Path(path)
    if not p.is_absolute():
        caller_file = inspect.stack()[1].filename
        p = Path(caller_file).parent / p
    return JsSource(p.read_text(encoding="utf-8"))


def inline_js(code: str) -> JsSource:
    """Define JavaScript inline."""
    return JsSource(code)


def _build_expression(js_source: JsSource, params: dict[str, Any]) -> str:
    """Build a full JS expression with a ``__params`` preamble.

    Wraps in an async IIFE so ``const`` declarations don't leak and
    ``await`` can be used inside the snippet.
    """
    params_json = json.dumps(params, default=str)
    return f"(async () => {{ const __params = {params_json}; {js_source.js} }})()"


class ActionNode(ABC):
    """Base class for all browser actions.

    Subclasses override :meth:`run` to execute against a tab.
    """

    @abstractmethod
    async def run(self, tab: Tab) -> object:
        """Execute this action against *tab*. Returns the JS/CDP result."""
        ...

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"


class JsActionNode(ActionNode):
    """Action backed by a JavaScript snippet.

    Subclasses set the ``js`` class attribute (via :func:`load_js` or
    :func:`inline_js`) and store their parameters as instance attributes
    in ``__init__``.  By default, :meth:`params` returns all instance
    attributes (via ``vars(self)``), so most subclasses don't need to
    override it.

    Example::

        class ClickAt(JsActionNode):
            js = inline_js('''
                const el = document.elementFromPoint(__params.x, __params.y);
                if (el) el.click();
                return el ? el.tagName : null;
            ''')

            def __init__(self, x: int, y: int) -> None:
                self.x = x
                self.y = y
    """

    js: JsSource

    def params(self) -> dict[str, Any]:
        """Return the parameters to inject as ``__params`` in JS.

        Defaults to ``vars(self)`` — override only if you need to
        transform or filter attributes.
        """
        return vars(self)

    async def run(self, tab: JsTab) -> object:
        expression = _build_expression(self.js, self.params())
        return await tab.evaluate_js(expression)

    def __repr__(self) -> str:
        cls = type(self).__name__
        p = self.params()
        args = ", ".join(f"{k}={v!r}" for k, v in p.items())
        return f"{cls}({args})"


# CdpActionNode was removed — CDP actions subclass ActionNode directly.
