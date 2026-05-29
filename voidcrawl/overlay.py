"""Standardized on-page debug overlay — a step banner + element highlight.

A reusable annotation layer for recordings, demos, and live debugging: a
pinned, click-through banner that narrates the current step, plus an
outline-and-label that points at the element a step acts on. It is pure
JavaScript injected through the page's ``evaluate_js``, so it works on any
:class:`~voidcrawl.Page` or :class:`~voidcrawl.PooledTab` and adds nothing to
the Rust core. Every overlay element uses ``pointer-events:none`` and so never
intercepts a click.

    from voidcrawl import BrowserSession, BrowserConfig, record
    from voidcrawl.overlay import Overlay

    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page("about:blank")
        overlay = Overlay(page)
        async with record(page, "demo.mp4"):
            await page.goto("https://example.com")
            await overlay.banner("Step 1/2 — open the page")
            await overlay.highlight("h1", label="title")
            ...
            await overlay.clear()

Overlays are appended to ``document.body``, so a full navigation clears them —
just call :meth:`banner` / :meth:`highlight` again after the page loads (each
call re-creates its element if missing). This is intentionally separate from
the step-debugger's transient red flash (:mod:`voidcrawl.debug`), which has a
different, ephemeral lifecycle.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from collections.abc import Awaitable

# Element ids are namespaced so the overlay never collides with page markup and
# is trivial to find/remove.
_BANNER_ID = "__vc_overlay_banner"
_MARK_ID = "__vc_overlay_mark"

# Default palette: a dark, translucent bar with a neon-green accent — high
# contrast over arbitrary pages and unmistakably "instrumentation, not content".
_DEFAULT_ACCENT = "#39ff14"
_DEFAULT_BACKGROUND = "rgba(8,10,14,.93)"


class _Evaluable(Protocol):
    """Anything with an async ``evaluate_js`` — :class:`Page` or :class:`PooledTab`."""

    def evaluate_js(self, expression: str) -> Awaitable[object]: ...


class Overlay:
    """A reusable on-page banner + element-highlight overlay bound to a page.

    Args:
        page: The :class:`~voidcrawl.Page` / :class:`~voidcrawl.PooledTab` (or
            any object with an async ``evaluate_js``) to draw on.
        accent: CSS color for the banner text, outline, and label background.
        background: CSS color for the banner's background bar.
    """

    def __init__(
        self,
        page: _Evaluable,
        *,
        accent: str = _DEFAULT_ACCENT,
        background: str = _DEFAULT_BACKGROUND,
    ) -> None:
        self._page = page
        self._accent = accent
        self._background = background

    async def banner(self, text: str) -> None:
        """Show (or update in place) the pinned step banner across the top."""
        css = (
            "position:fixed;top:0;left:0;right:0;z-index:2147483647;"
            "pointer-events:none;"
            f"background:{self._background};color:{self._accent};"
            "font:600 15px ui-monospace,SFMono-Regular,Menlo,monospace;"
            "padding:10px 16px;letter-spacing:.4px;"
            "box-shadow:0 2px 12px rgba(0,0,0,.5)"
        )
        js = f"""
(() => {{
  let b = document.getElementById({json.dumps(_BANNER_ID)});
  if (!b) {{
    b = document.createElement('div');
    b.id = {json.dumps(_BANNER_ID)};
    b.style.cssText = {json.dumps(css)};
    (document.body || document.documentElement).appendChild(b);
  }}
  b.textContent = {json.dumps(text)};
}})()
"""
        await self._page.evaluate_js(js)

    async def highlight(self, selector: str, *, label: str | None = None) -> bool:
        """Outline the first element matching *selector*, with an optional caption.

        Scrolls the element into view, then draws a fixed-position box around it
        (so it tracks the current viewport position). Replaces any prior
        highlight. Returns ``True`` if the element was found, ``False`` otherwise
        — never raises on a missing selector, so it is safe to call optimistically.
        """
        js = f"""
(() => {{
  document.getElementById({json.dumps(_MARK_ID)})?.remove();
  const el = document.querySelector({json.dumps(selector)});
  if (!el) return false;
  el.scrollIntoView({{block: 'center', behavior: 'instant'}});
  const r = el.getBoundingClientRect();
  const accent = {json.dumps(self._accent)};
  const box = document.createElement('div');
  box.id = {json.dumps(_MARK_ID)};
  box.style.cssText = [
    'position:fixed', 'z-index:2147483646', 'pointer-events:none',
    `left:${{r.left - 4}}px`, `top:${{r.top - 4}}px`,
    `width:${{r.width + 8}}px`, `height:${{r.height + 8}}px`,
    `border:3px solid ${{accent}}`, 'border-radius:6px',
    `box-shadow:0 0 0 3px ${{accent}}40`,
  ].join(';');
  const label = {json.dumps(label)};
  if (label) {{
    const tag = document.createElement('div');
    tag.textContent = label;
    tag.style.cssText = [
      'position:absolute', 'left:0', 'top:-24px',
      `background:${{accent}}`, 'color:#08100a',
      'font:600 12px ui-monospace,monospace', 'padding:2px 8px',
      'border-radius:4px', 'white-space:nowrap',
    ].join(';');
    box.appendChild(tag);
  }}
  (document.body || document.documentElement).appendChild(box);
  return true;
}})()
"""
        return bool(await self._page.evaluate_js(js))

    async def clear_highlight(self) -> None:
        """Remove the element highlight, leaving the banner in place."""
        mark = json.dumps(_MARK_ID)
        await self._page.evaluate_js(
            f"(() => {{ document.getElementById({mark})?.remove(); }})()"
        )

    async def clear(self) -> None:
        """Remove every overlay element (banner and highlight)."""
        await self._page.evaluate_js(
            f"""
(() => {{
  for (const id of [{json.dumps(_BANNER_ID)}, {json.dumps(_MARK_ID)}]) {{
    document.getElementById(id)?.remove();
  }}
}})()
"""
        )
