"""Input/form actions -- JS-tier and CDP-tier."""

from __future__ import annotations

from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, inline_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab


class SetInputValue(JsActionNode):
    """Bulk-set an input's value and fire ``input``/``change`` events.

    This does **not** simulate individual keystrokes — use
    :class:`CdpTypeText` for that.
    """

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
el.focus();
el.value = __params.text;
el.dispatchEvent(new Event('input', {bubbles: true}));
el.dispatchEvent(new Event('change', {bubbles: true}));
return null;
""")

    def __init__(self, selector: str, text: str) -> None:
        self.selector = selector
        self.text = text


class ClearInput(JsActionNode):
    """Clear an input field via JS."""

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
el.value = '';
el.dispatchEvent(new Event('input', {bubbles: true}));
return null;
""")

    def __init__(self, selector: str) -> None:
        self.selector = selector


class SelectOption(JsActionNode):
    """Select a ``<select>`` option by value via JS."""

    js = inline_js("""\
const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
el.value = __params.value;
el.dispatchEvent(new Event('change', {bubbles: true}));
return null;
""")

    def __init__(self, selector: str, value: str) -> None:
        self.selector = selector
        self.value = value


class CdpTypeText(ActionNode):
    """Type text character-by-character via CDP key events."""

    def __init__(self, text: str) -> None:
        self.text = text

    async def run(self, tab: Tab) -> None:
        for ch in self.text:
            await tab.dispatch_key_event("keyDown", key=ch, text=ch)
            await tab.dispatch_key_event("keyUp", key=ch)

    def __repr__(self) -> str:
        return f"CdpTypeText(text={self.text!r})"
