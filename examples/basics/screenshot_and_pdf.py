"""Capture screenshots (PNG) of a JS-rendered page."""

import asyncio
from pathlib import Path

from voidcrawl import BrowserPool, PoolConfig

OUTPUT_DIR = Path("output")
TARGET_URL = "https://qscrape.dev/l2/eshop"


async def _capture() -> None:
    """Open VaultMart and save a PNG screenshot after the page fully hydrates."""
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        # goto() waits for network idle — essential for Astro client:only islands.
        await tab.goto(TARGET_URL)

        # PNG screenshot
        png_bytes = await tab.screenshot_png()
        png_path = OUTPUT_DIR / "vaultmart.png"
        png_path.write_bytes(png_bytes)
        print(f"Screenshot saved: {png_path} ({len(png_bytes)} bytes)")


def main() -> None:
    """Capture a PNG screenshot of qscrape.dev/l2/eshop (VaultMart)."""
    OUTPUT_DIR.mkdir(exist_ok=True)
    asyncio.run(_capture())


if __name__ == "__main__":
    main()
