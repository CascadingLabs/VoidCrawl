"""Variable CDP viewport, live and concurrent: mobile/tablet/desktop
presets, custom dimensions, arbitrary bbox crops, viewport-only capture,
and scroll-then-crop — all exercised against real websites over a 4-tab
pool, to prove the feature under real concurrency, not just unit tests.

Run: .venv/bin/python examples/advanced/viewport_stress_test.py
"""

import asyncio
import re
import time
from pathlib import Path
from typing import Any

from voidcrawl import BrowserPool, PoolConfig
from voidcrawl.viewport import Viewport, list_device_presets

OUT_DIR = Path(__file__).parent / "screenshots"

# Each job: (label, url, Viewport, extra kwargs for tab.screenshot()).
# Mixes phone / tablet / desktop presets, a hand-rolled custom viewport,
# viewport-only capture, and an arbitrary bbox crop after scrolling — the
# full surface in one concurrent batch.
#
# Mobile/tablet presets are pointed at books.toscrape.com — a scraping
# sandbox site (genuinely responsive: `<meta name="viewport"
# content="width=device-width">`, and built to tolerate automated
# traffic). Wikipedia's own desktop domain, by contrast, hardcodes
# `<meta name="viewport" content="width=1120">` — a page-authored fixed
# layout width Chrome always respects over device emulation, so a mobile
# preset there renders at ~1120px, not the device's real width. That's
# correct browser behavior, not a bug in this feature; it just makes
# Wikipedia a poor site for *this* part of the demo. (An earlier version of
# this script also hit www.python.org for variety, but it started
# rate-limiting after a few runs in the same session — stick to sandbox
# sites for repeatable local runs.)
BOOKS = "https://books.toscrape.com/"
BOOKS_MYSTERY = BOOKS + "catalogue/category/books/mystery_3/"

JOBS: list[tuple[str, str, Viewport, dict[str, Any]]] = [
    ("mobile_iphone", BOOKS, Viewport(preset="iPhone 16 Pro Max"), {}),
    ("mobile_pixel", BOOKS_MYSTERY, Viewport(preset="Pixel 7"), {}),
    ("tablet_ipad", BOOKS, Viewport(preset="iPad Pro 11"), {}),
    ("tablet_android", BOOKS, Viewport(preset="Tablet"), {}),
    (
        "desktop_1080p",
        "https://en.wikipedia.org/wiki/1080p",
        Viewport(preset="Desktop 1080p"),
        {},
    ),
    (
        "custom_viewport",
        "https://en.wikipedia.org/wiki/Viewport",
        Viewport(width=1024, height=640),
        {},
    ),
    (
        "viewport_only",
        "https://en.wikipedia.org/wiki/Responsive_web_design",
        Viewport(preset="Desktop 1440p"),
        {"full_page": False},
    ),
    (
        "bbox_arbitrary_crop",
        "https://en.wikipedia.org/wiki/Cropping_(image)",
        Viewport(width=1920, height=1080),
        {"bbox": (860, 40, 400, 300)},
    ),
    (
        "scroll_then_crop",
        "https://en.wikipedia.org/wiki/Scrolling",
        Viewport(width=1280, height=800),
        {"scroll_viewports": 2.0, "bbox": (0, 0, 500, 300)},
    ),
]


def slug(label: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", label).strip("_")


def png_dimensions(png: bytes) -> tuple[int, int]:
    width = int.from_bytes(png[16:20], "big")
    height = int.from_bytes(png[20:24], "big")
    return width, height


async def run_job(
    pool: BrowserPool, label: str, url: str, vp: Viewport, extra: dict[str, Any]
) -> dict[str, Any]:
    started = time.monotonic()
    async with pool.acquire() as tab:
        resp = await tab.goto(url, timeout=30.0)
        title = await tab.title()
        # PooledTab.screenshot()'s many optional, heterogeneously-typed
        # kwargs can't be verified against a **dict[str, X] unpack.
        png = await tab.screenshot(**vp.as_kwargs(prefix="viewport_"), **extra)  # type: ignore[arg-type]

    assert isinstance(png, bytes), "no path= kwarg was passed, so this is always bytes"
    out_path = OUT_DIR / f"{slug(label)}.png"
    out_path.write_bytes(png)
    w, h = png_dimensions(png)

    return {
        "label": label,
        "url": url,
        "status": resp.status_code,
        "title": title,
        "png_dims": (w, h),
        "bytes": len(png),
        "path": out_path,
        "seconds": round(time.monotonic() - started, 2),
    }


async def main() -> None:
    OUT_DIR.mkdir(exist_ok=True)

    print("Available device presets (DevTools-style dropdown, as data):")
    for p in list_device_presets():
        dims = f"{p.width}x{p.height}"
        mobile = "mobile" if p.mobile else "desktop"
        print(f"  {p.name:<20} {dims:<10} dsf={p.device_scale_factor:<4} {mobile}")
    print()

    config = PoolConfig(browsers=1, tabs_per_browser=4)
    async with BrowserPool(config) as pool:
        started = time.monotonic()
        results = await asyncio.gather(
            *(run_job(pool, label, url, vp, extra) for label, url, vp, extra in JOBS)
        )
        elapsed = time.monotonic() - started

    for r in results:
        dims = f"{r['png_dims'][0]}x{r['png_dims'][1]}"
        print(
            f"[{r['status']}] {r['seconds']:>5.2f}s  {dims:<11}  "
            f"{r['bytes']:>9,}B  {r['path'].name:<28} {r['title']!r}"
        )
    tabs = config.tabs_per_browser
    print(f"\n{len(JOBS)} jobs in {elapsed:.2f}s over {tabs} tabs (1 headless Chrome)")
    print(f"-> {OUT_DIR}/")


if __name__ == "__main__":
    asyncio.run(main())
