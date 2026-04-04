"""DOM query and mutation actions (JS-tier).

Provides :class:`GetAttribute`, :class:`GetText`, :class:`QueryAll`,
and :class:`SetAttribute` for reading and writing DOM element properties.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Generic, TypeVar, cast, overload

from voidcrawl.actions._base import JsActionNode, inline_js
from voidcrawl.schema import Schema

if TYPE_CHECKING:
    from voidcrawl.actions._protocol import JsTab

_T = TypeVar("_T")
_C = TypeVar("_C", bound="Schema")


class GetAttribute(JsActionNode):
    """Read an HTML attribute from the first matching element.

    Returns ``None`` if the element is not found.  The result is
    available as the return value of :meth:`run` (``str | None``).

    Args:
        selector: CSS selector targeting the element.
        attr: Attribute name (e.g. ``"href"``, ``"data-id"``).
    """

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) return null;
return el.getAttribute(__params.attr);
""")

    def __init__(self, selector: str, attr: str) -> None:
        self.selector = selector
        self.attr = attr


class GetText(JsActionNode):
    """Read ``textContent`` from the first matching element.

    Returns ``None`` if the element is not found.  The result is
    available as the return value of :meth:`run` (``str | None``).

    Args:
        selector: CSS selector targeting the element.
    """

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) return null;
return (el.textContent ?? '').trim() || null;
""")

    def __init__(self, selector: str) -> None:
        self.selector = selector


class QueryAll(JsActionNode, Generic[_T]):
    """Query all elements matching *selector* and extract fields from each.

    Each entry in *fields* maps a result key to either:

    * a CSS sub-selector string — the matched element's ``textContent``
      (trimmed) is returned, or ``None`` if nothing matches.
    * a ``(sub_selector, attr)`` tuple — the named attribute of the
      matched element is returned, or ``None`` if nothing matches.

    Pass an empty string ``""`` as the sub-selector to target the root
    element itself rather than a descendant.

    Pass a :class:`~voidcrawl.schema.Schema` subclass as *fields*
    to receive typed model instances instead of raw dicts.

    Args:
        selector: CSS selector for the root elements to iterate over.
        fields: Mapping of result key → sub-selector or
            ``(sub_selector, attribute)`` tuple, **or** a
            :class:`~voidcrawl.schema.Schema` subclass whose field
            declarations are used automatically.

    Example — raw dicts::

        QueryAll(
            ".article",
            {"title": "h2", "url": ("a", "href"), "date": ".byline"},
        )

    Example — typed Schema::

        class Article(vc.Schema):
            title: str = vc.Text("h2")
            url: str | None = vc.Attr("a", "href")


        QueryAll(".article", Article)
    """

    js = inline_js("""\
const items = Array.from(document.querySelectorAll(__params.selector));
return items.map(item => {
    const result = {};
    for (const [name, spec] of Object.entries(__params.fields)) {
        if (Array.isArray(spec)) {
            const [sub, attr] = spec;
            const el = sub ? item.querySelector(sub) : item;
            result[name] = el ? el.getAttribute(attr) : null;
        } else {
            const el = spec ? item.querySelector(spec) : item;
            result[name] = el ? (el.textContent ?? '').trim() || null : null;
        }
    }
    return result;
});
""")

    @overload
    def __new__(
        cls,
        selector: str,
        fields: dict[str, str | tuple[str, str]],
    ) -> QueryAll[dict[str, str | None]]: ...

    @overload
    def __new__(cls, selector: str, fields: type[_C]) -> QueryAll[_C]: ...

    def __new__(cls, *_args: Any, **_kwargs: Any) -> QueryAll[Any]:
        return super().__new__(cls)

    def __init__(
        self,
        selector: str,
        fields: dict[str, str | tuple[str, str]] | type[Schema],
    ) -> None:
        self.selector = selector
        if isinstance(fields, type) and issubclass(fields, Schema):
            self._schema_cls: type[Schema] | None = fields
            self.fields = fields._vc_fields_spec()
        else:
            self._schema_cls = None
            self.fields = fields

    def params(self) -> dict[str, Any]:
        return {"selector": self.selector, "fields": self.fields}

    async def run(self, tab: JsTab) -> list[_T]:
        raw: list[dict[str, str | None]] = await super().run(tab)  # type: ignore[assignment]
        if self._schema_cls is not None:
            return cast("list[_T]", [self._schema_cls(**row) for row in raw])
        return cast("list[_T]", raw)


class SetAttribute(JsActionNode):
    """Set an HTML attribute on the first matching element.

    Raises a JS ``Error`` if no element matches the selector.

    Args:
        selector: CSS selector targeting the element.
        attr: Attribute name to set.
        value: Attribute value to assign.
    """

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
el.setAttribute(__params.attr, __params.value);
return null;
""")

    def __init__(self, selector: str, attr: str, value: str) -> None:
        self.selector = selector
        self.attr = attr
        self.value = value
