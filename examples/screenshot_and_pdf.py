"""Capture screenshots (PNG) and PDF exports of a page."""

import asyncio
from pathlib import Path

from void_crawl import BrowserPool, PoolConfig

OUTPUT_DIR = Path("output")


async def _capture() -> None:
    """Open example.com and save a PNG screenshot and PDF export."""
    async with BrowserPool(PoolConfig()) as pool:
        async with pool.acquire() as tab:
            await tab.navigate("https://example.com")

            # PNG screenshot
            png_bytes = await tab.screenshot_png()
            png_path = OUTPUT_DIR / "example.png"
            png_path.write_bytes(png_bytes)
            print(f"Screenshot saved: {png_path} ({len(png_bytes)} bytes)")


def main() -> None:
    """Capture a PNG screenshot of example.com."""
    OUTPUT_DIR.mkdir(exist_ok=True)
    asyncio.run(_capture())


if __name__ == "__main__":
    main()
