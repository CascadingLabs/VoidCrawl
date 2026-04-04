"""Type stubs for voidcrawl.actions.builtin.dom."""

from __future__ import annotations

from typing import Generic, TypeVar, overload

from voidcrawl.actions._base import JsActionNode
from voidcrawl.actions._protocol import JsTab
from voidcrawl.schema import Schema

__all__ = ["GetAttribute", "GetText", "QueryAll", "SetAttribute"]

_T = TypeVar("_T")
_C = TypeVar("_C", bound=Schema)

class GetAttribute(JsActionNode):
    """Read an HTML attribute from the first matching element.

    Returns ``None`` if the element is not found.
    """

    selector: str
    attr: str

    def __init__(self, selector: str, attr: str) -> None: ...
    async def run(self, tab: JsTab) -> str | None: ...

class GetText(JsActionNode):
    """Read ``textContent`` from the first matching element.

    Returns ``None`` if the element is not found.
    """

    selector: str

    def __init__(self, selector: str) -> None: ...
    async def run(self, tab: JsTab) -> str | None: ...

class QueryAll(JsActionNode, Generic[_T]):
    """Query all elements matching *selector* and extract fields from each.

    Pass a :class:`~voidcrawl.schema.Schema` subclass as *fields* to
    receive typed model instances, or a plain dict mapping for raw results.

    Example — typed Schema::

        class Article(vc.Schema):
            title: str = vc.Text("h2")
            url: str | None = vc.Attr("a", "href")

        articles = await QueryAll(".article", Article).run(page)
        # articles: list[Article]

    Example — raw dicts::

        results = await QueryAll(
            ".article",
            {"title": "h2", "url": ("a", "href")},
        ).run(page)
        # results: list[dict[str, str | None]]
    """

    selector: str
    fields: dict[str, str | tuple[str, str]]

    @overload
    def __init__(
        self: QueryAll[dict[str, str | None]],
        selector: str,
        fields: dict[str, str | tuple[str, str]],
    ) -> None: ...
    @overload
    def __init__(
        self: QueryAll[_C],
        selector: str,
        fields: type[_C],
    ) -> None: ...
    async def run(self, tab: JsTab) -> list[_T]: ...

class SetAttribute(JsActionNode):
    """Set an HTML attribute on the first matching element.

    Raises a JS ``Error`` if no element matches the selector.
    """

    selector: str
    attr: str
    value: str

    def __init__(self, selector: str, attr: str, value: str) -> None: ...
    async def run(self, tab: JsTab) -> None: ...
