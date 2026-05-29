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
import os
from pathlib import Path
from typing import Any

from voidcrawl import BrowserConfig, BrowserSession, record
from voidcrawl.overlay import Overlay

HEADLESS = os.getenv("HEADFUL") != "1"
OUT = Path(__file__).parent / ".voidcrawl" / "record"
QUERY = "coffee"

# (label, latitude, longitude, IANA timezone, locale)
TOUR = [
    ("New York", 40.7580, -73.9855, "America/New_York", "en-US"),
    ("Paris", 48.8584, 2.2945, "Europe/Paris", "fr-FR"),
    ("Tokyo", 35.6595, 139.7004, "Asia/Tokyo", "ja-JP"),
]


async def run_tour(page: Any) -> None:
    # The standardized overlay: a pinned step banner + element highlighting,
    # no hand-rolled JS. A full navigation clears it, so we re-banner per step.
    overlay = Overlay(page)
    total = len(TOUR)
    for k, (city, lat, lon, tz, locale) in enumerate(TOUR, 1):
        prefix = f"● REC · voidcrawl teleport · Step {k}/{total}"

        # 1. Teleport: override geolocation + timezone + locale for this city.
        await page.set_geolocation(lat, lon)
        await page.set_timezone(tz)
        await page.set_locale(locale)
        await overlay.banner(f"{prefix} — Teleport to {city}  ({lat:.3f}, {lon:.3f})")
        await asyncio.sleep(1.2)

        # 2. Open Google Maps centered on the teleported coordinates.
        await overlay.banner(f"{prefix} — Open Google Maps · '{QUERY}' near {city}")
        url = f"https://www.google.com/maps/search/{QUERY}/@{lat},{lon},14z?hl=en"
        try:
            await page.goto(url, timeout=30.0)
        except Exception as exc:  # demo: never let one city abort the tour
            print(f"  {city}: maps load failed ({exc})")
        await asyncio.sleep(2.5)  # let tiles + results panel render
        await overlay.banner(f"{prefix} — '{QUERY}' near {city}")

        # 3. Highlight the search box, then the nearby-results feed.
        await overlay.highlight("#searchboxinput", label="search")
        await asyncio.sleep(1.2)
        await overlay.highlight('[role="feed"]', label=f"{QUERY} near {city}")
        await asyncio.sleep(1.8)
        await overlay.clear_highlight()
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
