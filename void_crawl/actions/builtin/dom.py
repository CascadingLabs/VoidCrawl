"""DOM query/mutation actions (JS-tier)."""

from __future__ import annotations

from void_crawl.actions._base import JsActionNode, inline_js


class GetAttribute(JsActionNode):
    """Get an attribute value from an element."""

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) return null;
return el.getAttribute(__params.attr);
""")

    def __init__(self, selector: str, attr: str) -> None:
        self.selector = selector
        self.attr = attr


class GetText(JsActionNode):
    """Get ``textContent`` from an element."""

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) return null;
return el.textContent;
""")

    def __init__(self, selector: str) -> None:
        self.selector = selector


class SetAttribute(JsActionNode):
    """Set an attribute on an element."""

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
