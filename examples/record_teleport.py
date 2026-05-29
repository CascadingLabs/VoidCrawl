"""Record a Google Maps teleport tour to mp4/gif — native screencast demo.

Teleports the browser across the world via CDP geolocation override and, at
each stop, opens Google Maps centered on that city and searches for coffee.
A pinned on-page HUD narrates the current step (``● REC · Step k/N — …``) and
debug highlighting outlines the element each step acts on, so the finished
video reads as a guided walkthrough.

The whole tour is wrapped in :func:`voidcrawl.record`, which captures a real
CDP screencast (frames pushed only when the page changes, all at a uniform
viewport size) and stitches them with ffmpeg — no screenshot-per-step loop, no
mixed-size canvas normalization. That hand-rolled path is exactly what this
native recorder replaces.

    uv run python examples/record_teleport.py
    HEADFUL=1 uv run python examples/record_teleport.py   # watch live (needs a display)

Output: ``.voidcrawl/record/maps_teleport.mp4`` (+ ``.gif``).

Parallelizing: each tab records independently — screencast frames are routed
per CDP target — so to record several cities at once, acquire N
:class:`~voidcrawl.PooledTab` s from a :class:`~voidcrawl.BrowserPool` and run
one ``record(tab, f"city_{i}.mp4")`` block per tab under ``asyncio.gather``.
Their frame streams never interleave. This example stays sequential so the
output reads top-to-bottom like a tour.
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any

from voidcrawl import BrowserConfig, BrowserSession, record

HEADLESS = os.getenv("HEADFUL") != "1"
OUT = Path(__file__).parent / ".voidcrawl" / "record"
QUERY = "coffee"

# (label, latitude, longitude, IANA timezone, locale)
TOUR = [
    ("New York", 40.7580, -73.9855, "America/New_York", "en-US"),
    ("Paris", 48.8584, 2.2945, "Europe/Paris", "fr-FR"),
    ("Tokyo", 35.6595, 139.7004, "Asia/Tokyo", "ja-JP"),
]

# A pinned, click-through header bar that narrates the current step. Rewritten
# in place each step (never blocks the page — pointer-events:none).
_HEADER_JS = """
(() => {
  let b = document.getElementById('__vc_hud');
  if (!b) {
    b = document.createElement('div');
    b.id = '__vc_hud';
    b.style.cssText = [
      'position:fixed', 'top:0', 'left:0', 'right:0', 'z-index:2147483647',
      'pointer-events:none', 'background:rgba(8,10,14,.93)', 'color:#39ff14',
      'font:600 15px ui-monospace,SFMono-Regular,Menlo,monospace',
      'padding:10px 16px', 'letter-spacing:.4px',
      'box-shadow:0 2px 12px rgba(0,0,0,.5)',
    ].join(';');
    document.body.appendChild(b);
  }
  b.textContent = __TEXT__;
})()
"""

# Outline an element and float a small caption above it (debug highlighting).
# Best-effort: silently no-ops if the selector isn't present yet.
_HIGHLIGHT_JS = """
(() => {
  document.getElementById('__vc_mark')?.remove();
  const el = document.querySelector(__SELECTOR__);
  if (!el) return false;
  el.scrollIntoView({block: 'center', behavior: 'instant'});
  const r = el.getBoundingClientRect();
  const box = document.createElement('div');
  box.id = '__vc_mark';
  box.style.cssText = [
    'position:fixed', 'z-index:2147483646', 'pointer-events:none',
    `left:${r.left - 4}px`, `top:${r.top - 4}px`,
    `width:${r.width + 8}px`, `height:${r.height + 8}px`,
    'border:3px solid #39ff14', 'border-radius:6px',
    'box-shadow:0 0 0 3px rgba(57,255,20,.25)',
  ].join(';');
  const tag = document.createElement('div');
  tag.textContent = __LABEL__;
  tag.style.cssText = [
    'position:absolute', 'left:0', 'top:-24px',
    'background:#39ff14', 'color:#08100a',
    'font:600 12px ui-monospace,monospace', 'padding:2px 8px',
    'border-radius:4px', 'white-space:nowrap',
  ].join(';');
  box.appendChild(tag);
  document.body.appendChild(box);
  return true;
})()
"""

_CLEAR_HIGHLIGHT_JS = "(() => { document.getElementById('__vc_mark')?.remove(); })()"


async def set_header(page: Any, text: str) -> None:
    await page.evaluate_js(_HEADER_JS.replace("__TEXT__", json.dumps(text)))


async def highlight(page: Any, selector: str, label: str) -> None:
    js = _HIGHLIGHT_JS.replace("__SELECTOR__", json.dumps(selector)).replace(
        "__LABEL__", json.dumps(label)
    )
    await page.evaluate_js(js)


async def clear_highlight(page: Any) -> None:
    await page.evaluate_js(_CLEAR_HIGHLIGHT_JS)


async def run_tour(page: Any) -> None:
    total = len(TOUR)
    for k, (city, lat, lon, tz, locale) in enumerate(TOUR, 1):
        prefix = f"● REC · voidcrawl teleport · Step {k}/{total}"

        # 1. Teleport: override geolocation + timezone + locale for this city.
        await page.set_geolocation(lat, lon)
        await page.set_timezone(tz)
        await page.set_locale(locale)
        await set_header(page, f"{prefix} — Teleport to {city}  ({lat:.3f}, {lon:.3f})")
        await asyncio.sleep(1.2)

        # 2. Open Google Maps centered on the teleported coordinates.
        await set_header(page, f"{prefix} — Open Google Maps · '{QUERY}' near {city}")
        url = f"https://www.google.com/maps/search/{QUERY}/@{lat},{lon},14z?hl=en"
        try:
            await page.goto(url, timeout=30.0)
        except Exception as exc:  # demo: never let one city abort the tour
            print(f"  {city}: maps load failed ({exc})")
        await asyncio.sleep(2.5)  # let tiles + results panel render
        await set_header(page, f"{prefix} — '{QUERY}' near {city}")

        # 3. Highlight the search box, then the nearby-results feed.
        await highlight(page, "#searchboxinput", "search")
        await asyncio.sleep(1.2)
        await highlight(page, '[role="feed"]', f"{QUERY} near {city}")
        await asyncio.sleep(1.8)
        await clear_highlight(page)
        print(f"  {city}: recorded")


async def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    # Fixed window size → uniform, predictable video frames.
    cfg = BrowserConfig(
        headless=HEADLESS,
        stealth=True,
        no_sandbox=True,
        extra_args=["--window-size=1280,860"],
    )
    out = OUT / "maps_teleport.mp4"
    async with BrowserSession(cfg) as browser:
        page = await browser.new_page("about:blank")
        async with record(page, out, fps=12, also_gif=True, max_width=1280) as rec:
            await run_tour(page)
        await page.close()

    print(f"\ncaptured {rec.frame_count} frames")
    if rec.mp4_path:
        print(f"MP4 → {rec.mp4_path}  ({rec.mp4_path.stat().st_size // 1024} KB)")
    if rec.gif_path:
        print(f"GIF → {rec.gif_path}  ({rec.gif_path.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    asyncio.run(main())
