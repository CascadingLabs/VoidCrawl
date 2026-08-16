"""Screenshot 10 Wikipedia pages concurrently over 4 tabs in one headless Chrome.

One `BrowserPool` launches a single headless Chrome process
(`browsers=1`) and hands out up to `tabs_per_browser=4` tabs at a time.
`asyncio.gather` submits all 10 URLs at once; the pool's semaphore lets
only 4 run concurrently and queues the rest. Each tab navigates, then
captures a full-page PNG and writes it to `OUT_DIR`.
"""

import asyncio
import re
import time
from pathlib import Path

from voidcrawl import BrowserPool, PoolConfig

OUT_DIR = Path(__file__).parent / "screenshots"

PAGES = [
    "https://en.wikipedia.org/wiki/Python_(programming_language)",
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://en.wikipedia.org/wiki/Chromium_(web_browser)",
    "https://en.wikipedia.org/wiki/Web_scraping",
    "https://en.wikipedia.org/wiki/Async/await",
    "https://en.wikipedia.org/wiki/HTTP",
    "https://en.wikipedia.org/wiki/JSON",
    "https://en.wikipedia.org/wiki/Concurrency_(computer_science)",
    "https://en.wikipedia.org/wiki/Headless_browser",
    "https://en.wikipedia.org/wiki/Wikipedia",
]


def slug(url: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", url.rsplit("/", 1)[-1]).strip("_")


async def screenshot_one(pool: BrowserPool, url: str) -> dict:
    started = time.monotonic()
    async with pool.acquire() as tab:
        resp = await tab.goto(url, timeout=30.0)
        title = await tab.title()
        # Navigation/JS/render happen fully concurrently across tabs; the
        # pool serializes only the brief activate+capture instant per
        # browser process (headless Chrome only composites the foreground
        # tab), so this doesn't block the other 3 tabs' page work.
        png = await tab.screenshot_png()

    out_path = OUT_DIR / f"{slug(url)}.png"
    out_path.write_bytes(png)

    return {
        "url": url,
        "title": title,
        "status": resp.status_code,
        "bytes": len(png),
        "path": out_path,
        "seconds": round(time.monotonic() - started, 2),
    }


async def main() -> None:
    OUT_DIR.mkdir(exist_ok=True)
    config = PoolConfig(browsers=1, tabs_per_browser=4)
    async with BrowserPool(config) as pool:
        started = time.monotonic()
        results = await asyncio.gather(*(screenshot_one(pool, url) for url in PAGES))
        elapsed = time.monotonic() - started

    for r in results:
        print(
            f"[{r['status']}] {r['seconds']:>5.2f}s  {r['bytes']:>8,} bytes  "
            f"{r['path'].name:<45} {r['title']!r}"
        )
    print(
        f"\n{len(PAGES)} screenshots in {elapsed:.2f}s "
        f"over {config.tabs_per_browser} tabs "
        f"(1 headless Chrome process) -> {OUT_DIR}/"
    )


if __name__ == "__main__":
    asyncio.run(main())
