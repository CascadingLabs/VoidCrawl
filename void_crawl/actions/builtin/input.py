"""Input and form actions — JS-tier and CDP-tier.

Provides :class:`SetInputValue`, :class:`ClearInput`,
:class:`SelectOption`, and :class:`CdpTypeText` for interacting
with form elements.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from void_crawl.actions._base import ActionNode, JsActionNode, inline_js

if TYPE_CHECKING:
    from void_crawl.actions._protocol import Tab


class SetInputValue(JsActionNode):
    """Bulk-set an input's value and fire ``input``/``change`` events.

    This does **not** simulate individual keystrokes — use
    :class:`CdpTypeText` for realistic per-character typing.

    Args:
        selector: CSS selector targeting the ``<input>`` or ``<textarea>``.
        text: The value to assign.
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
    """Clear an input field and fire an ``input`` event via JS.

    Args:
        selector: CSS selector targeting the ``<input>`` or ``<textarea>``.
    """

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
    """Select a ``<select>`` option by value and fire a ``change`` event.

    Args:
        selector: CSS selector targeting the ``<select>`` element.
        value: The ``value`` attribute of the ``<option>`` to select.
    """

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
    """Type *text* character-by-character via CDP ``Input.dispatchKeyEvent``.

    Each character produces a ``keyDown``/``keyUp`` pair.  This is more
    realistic than :class:`SetInputValue` and triggers per-keystroke
    event listeners.

    Args:
        text: The string to type.
    """

    def __init__(self, text: str) -> None:
        self.text = text

    async def run(self, tab: Tab) -> None:
        """Dispatch ``keyDown``/``keyUp`` pairs for each character in *text*.

        Args:
            tab: Tab-like object to send the key events to.
        """
        for ch in self.text:
            await tab.dispatch_key_event("keyDown", key=ch, text=ch)
            await tab.dispatch_key_event("keyUp", key=ch)

    def __repr__(self) -> str:
        return f"CdpTypeText(text={self.text!r})"
