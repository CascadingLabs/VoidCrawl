"""Standardized on-page debug overlay — a step banner + element highlight.

A reusable annotation layer for recordings, demos, and live debugging: a
pinned, click-through banner that narrates the current step, plus
outline-and-label marks that point at the elements a step acts on. It is pure
JavaScript injected through the page's ``evaluate_js``, so it works on any
:class:`~voidcrawl.Page` or :class:`~voidcrawl.PooledTab` and adds nothing to
the Rust core. Every overlay element uses ``pointer-events:none`` and so never
intercepts a click.

    from voidcrawl import BrowserSession, BrowserConfig, record
    from voidcrawl.overlay import Overlay

    overlay = Overlay(page)
    async with record(page, "demo.mp4"):
        await page.goto("https://example.com")
        await overlay.banner("Step 1/2 — open the page")
        await overlay.highlight("h1", label="title")
        ...
        await overlay.clear()

Extending it (e.g. by a downstream caller like Yosoi):

* **Theme** — pass an :class:`OverlayStyle` to recolor, reposition (top/bottom),
  or restyle without touching any JS::

      Overlay(page, style=OverlayStyle(accent="#7c3aed", position="bottom"))

* **Restructure** — subclass and override a ``_*_css`` hook to restyle one
  element, or override :meth:`banner` / :meth:`highlight` to render something
  richer (a progress bar, a multi-field HUD) while reusing the injection plumbing.
* **Replace** — anything matching :class:`OverlayLike` can stand in wherever an
  overlay is accepted; the overlay holds no privileged state, so a caller can
  also just inject its own JS via ``page.evaluate_js`` and skip this entirely.

Multiple highlights can coexist via distinct ``key`` values. Overlays live in
``document.body``, so a full navigation clears them — call the methods again
after the page loads (each re-creates its element if missing). This is
intentionally separate from the step-debugger's transient red flash
(:mod:`voidcrawl.debug`), which has a different, ephemeral lifecycle.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, Protocol, runtime_checkable

if TYPE_CHECKING:
    from collections.abc import Awaitable

# Element ids are namespaced so overlays never collide with page markup and are
# trivial to find/remove. Highlight marks are keyed (``prefix + key``) so many
# can coexist.
_BANNER_ID = "__vc_overlay_banner"
_MARK_PREFIX = "__vc_overlay_mark__"


@dataclass(frozen=True)
class OverlayStyle:
    """Appearance of an :class:`Overlay`. All fields are plain CSS values.

    The defaults are a dark, translucent bar with a neon-green accent — high
    contrast over arbitrary pages and unmistakably "instrumentation, not
    content". ``outline_color`` / banner ``text_color`` fall back to ``accent``.

    Note: the highlight's outer glow appends an alpha to the outline color, so
    a 6-digit hex ``accent`` / ``outline_color`` renders best.
    """

    accent: str = "#39ff14"
    background: str = "rgba(8,10,14,.93)"
    position: Literal["top", "bottom"] = "top"
    font: str = "600 15px ui-monospace,SFMono-Regular,Menlo,monospace"
    padding: str = "10px 16px"
    text_color: str | None = None
    outline_color: str | None = None
    outline_width: int = 3
    outline_radius: int = 6
    label_font: str = "600 12px ui-monospace,monospace"
    label_text_color: str = "#08100a"


@runtime_checkable
class OverlayLike(Protocol):
    """Structural interface an overlay must satisfy to be swapped in by a caller."""

    async def banner(self, text: str) -> None: ...
    async def highlight(
        self, selector: str, *, label: str | None = ..., key: str = ...
    ) -> bool: ...
    async def clear_highlight(self, key: str | None = ...) -> None: ...
    async def clear(self) -> None: ...


class _Evaluable(Protocol):
    """Anything with an async ``evaluate_js`` — :class:`Page` or :class:`PooledTab`."""

    def evaluate_js(self, expression: str) -> Awaitable[object]: ...


class Overlay:
    """A reusable on-page banner + element-highlight overlay bound to a page.

    Args:
        page: The :class:`~voidcrawl.Page` / :class:`~voidcrawl.PooledTab` (or
            any object with an async ``evaluate_js``) to draw on.
        style: Full appearance control. If omitted, defaults are used, with the
            ``accent`` / ``background`` shortcuts applied on top when given.
        accent: Shortcut for ``OverlayStyle.accent`` (ignored if ``style`` set).
        background: Shortcut for ``OverlayStyle.background`` (ignored if ``style`` set).
    """

    def __init__(
        self,
        page: _Evaluable,
        *,
        style: OverlayStyle | None = None,
        accent: str | None = None,
        background: str | None = None,
    ) -> None:
        if style is None:
            style = OverlayStyle(
                accent=accent or OverlayStyle.accent,
                background=background or OverlayStyle.background,
            )
        self._page = page
        self._style = style

    # ── Overridable appearance hooks (return CSS text) ──────────────────────

    def _banner_css(self) -> str:
        s = self._style
        edge = "bottom:0" if s.position == "bottom" else "top:0"
        shadow_dir = "0 -2px 12px" if s.position == "bottom" else "0 2px 12px"
        return (
            f"position:fixed;{edge};left:0;right:0;z-index:2147483647;"
            "pointer-events:none;"
            f"background:{s.background};color:{s.text_color or s.accent};"
            f"font:{s.font};padding:{s.padding};letter-spacing:.4px;"
            f"box-shadow:{shadow_dir} rgba(0,0,0,.5)"
        )

    def _box_css(self) -> str:
        """Static part of a highlight box; JS prepends the computed position/size."""
        s = self._style
        color = s.outline_color or s.accent
        return (
            "position:fixed;z-index:2147483646;pointer-events:none;"
            f"border:{s.outline_width}px solid {color};"
            f"border-radius:{s.outline_radius}px;box-shadow:0 0 0 3px {color}40"
        )

    def _label_css(self) -> str:
        s = self._style
        color = s.outline_color or s.accent
        return (
            "position:absolute;left:0;top:-24px;"
            f"background:{color};color:{s.label_text_color};"
            f"font:{s.label_font};padding:2px 8px;border-radius:4px;white-space:nowrap"
        )

    @staticmethod
    def _mark_id(key: str) -> str:
        return f"{_MARK_PREFIX}{key}"

    # ── Public API ──────────────────────────────────────────────────────────

    async def banner(self, text: str) -> None:
        """Show (or update in place) the pinned step banner."""
        js = f"""
(() => {{
  let b = document.getElementById({json.dumps(_BANNER_ID)});
  if (!b) {{
    b = document.createElement('div');
    b.id = {json.dumps(_BANNER_ID)};
    b.style.cssText = {json.dumps(self._banner_css())};
    (document.body || document.documentElement).appendChild(b);
  }}
  b.textContent = {json.dumps(text)};
}})()
"""
        await self._page.evaluate_js(js)

    async def highlight(
        self, selector: str, *, label: str | None = None, key: str = "default"
    ) -> bool:
        """Outline the first element matching *selector*, with an optional caption.

        Scrolls the element into view, then draws a fixed-position box around it
        (so it tracks the current viewport position). Replaces any prior
        highlight **with the same** ``key``; pass distinct keys to show several at
        once. Returns ``True`` if the element was found, ``False`` otherwise —
        never raises on a missing selector, so it is safe to call optimistically.
        """
        js = f"""
(() => {{
  const id = {json.dumps(self._mark_id(key))};
  document.getElementById(id)?.remove();
  const el = document.querySelector({json.dumps(selector)});
  if (!el) return false;
  el.scrollIntoView({{block: 'center', behavior: 'instant'}});
  const r = el.getBoundingClientRect();
  const box = document.createElement('div');
  box.id = id;
  box.style.cssText =
    `left:${{r.left - 4}}px;top:${{r.top - 4}}px;`
    + `width:${{r.width + 8}}px;height:${{r.height + 8}}px;`
    + {json.dumps(self._box_css())};
  const label = {json.dumps(label)};
  if (label) {{
    const tag = document.createElement('div');
    tag.textContent = label;
    tag.style.cssText = {json.dumps(self._label_css())};
    box.appendChild(tag);
  }}
  (document.body || document.documentElement).appendChild(box);
  return true;
}})()
"""
        return bool(await self._page.evaluate_js(js))

    async def clear_highlight(self, key: str | None = None) -> None:
        """Remove one highlight by *key*, or all highlights when ``key`` is None.

        Leaves the banner in place.
        """
        if key is None:
            js = f"""
(() => {{
  for (const el of document.querySelectorAll('[id^="{_MARK_PREFIX}"]')) el.remove();
}})()
"""
        else:
            mark = json.dumps(self._mark_id(key))
            js = f"(() => {{ document.getElementById({mark})?.remove(); }})()"
        await self._page.evaluate_js(js)

    async def clear(self) -> None:
        """Remove every overlay element (banner and all highlights)."""
        js = f"""
(() => {{
  document.getElementById({json.dumps(_BANNER_ID)})?.remove();
  for (const el of document.querySelectorAll('[id^="{_MARK_PREFIX}"]')) el.remove();
}})()
"""
        await self._page.evaluate_js(js)
