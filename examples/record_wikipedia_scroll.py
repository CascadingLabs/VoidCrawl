"""Record a real Wikipedia viewport while scrolling and opening Contents.

Run from a build that includes the screen-recording API:
    ./build.sh
    uv run python examples/record_wikipedia_scroll.py

Writes a playable H.264 MP4 to ``output/wikipedia-recording/viewport.mp4``.
The build requires ``ffmpeg`` on ``PATH``; ``./build.sh`` enables the MP4
encoder for the Python extension.
"""

from __future__ import annotations

import asyncio
from pathlib import Path

from voidcrawl import BrowserConfig, BrowserSession

URL = "https://en.wikipedia.org/wiki/Browser_automation"
OUTPUT_DIR = Path("output/wikipedia-recording")


async def main() -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    async with BrowserSession(BrowserConfig(headless=True)) as browser:
        # A dedicated window avoids another tab occluding this screencast.
        page = await browser.new_page_in_window(URL)
        await page.wait_for_network_idle(timeout=15.0)

        # Record while driving the live page. This is viewport-only: CDP
        # screencasts capture what is composited on screen, not the full page.
        handle = await page.start_recording(
            duration_secs=20,
            fps=12,
            output_dir=str(OUTPUT_DIR),
            encode=["mp4"],
        )
        try:
            # Open the desktop Contents panel when that control is present.
            await page.evaluate_js(
                "document.querySelector('#vector-toc-collapsible-button')?.click()"
            )
            await asyncio.sleep(1)

            # Smooth scrolling produces visible, timestamped viewport changes.
            for _ in range(4):
                await page.evaluate_js(
                    "window.scrollBy({ top: Math.round(innerHeight * 0.75), "
                    "behavior: 'smooth' })"
                )
                await asyncio.sleep(1.5)
        finally:
            recording = await handle.stop()

    region = recording.regions[0]
    print(
        f"captured {recording.frames_captured} frames in {recording.duration_ms:.0f} ms"
    )
    print(
        f"effective FPS: {recording.effective_fps():.1f}; "
        f"dropped: {recording.frames_dropped}"
    )
    print(f"video: {region.outputs[0]}")


if __name__ == "__main__":
    asyncio.run(main())
