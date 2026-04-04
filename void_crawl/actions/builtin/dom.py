"""DOM query and mutation actions (JS-tier).

Provides :class:`GetAttribute`, :class:`GetText`, and
:class:`SetAttribute` for reading and writing DOM element properties.
"""

from __future__ import annotations

from void_crawl.actions._base import JsActionNode, inline_js


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
return el.textContent;
""")

    def __init__(self, selector: str) -> None:
        self.selector = selector


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
