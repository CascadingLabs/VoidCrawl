"""Record a page, and record two elements of it as separate regions.

Shows the three things that distinguish a recording from a screenshot:
selectors are plural (one region each, cut from a single screencast), `fps` is
a ceiling rather than a guarantee, and a page in its own browser window records
concurrently instead of holding the browser's capture lock.

Run with:
    uv run python examples/recording/screen_recording.py
"""

import asyncio
from pathlib import Path

from voidcrawl import BrowserConfig, BrowserSession

OUTPUT_DIR = Path("output/recording")
TARGET_URL = "https://qscrape.dev/l2/eshop"


async def _record_page() -> None:
    """Record the viewport, then record two elements as separate regions."""
    async with (
        BrowserSession(BrowserConfig()) as browser,
        browser.page() as page,
    ):
        await page.goto(TARGET_URL)

        # Whole viewport. `fps` is a ceiling: Chrome emits frames when it
        # paints, so a mostly-static page yields far fewer than requested.
        rec = await page.record(duration_secs=4, fps=10, output_dir=str(OUTPUT_DIR))
        print(
            f"viewport: {rec.frames_captured} frames "
            f"({rec.effective_fps():.1f} fps effective, {rec.frames_dropped} dropped)"
        )

        # Two regions, one screencast. Each selector is resolved to a rectangle
        # once at the start and then held fixed.
        multi = await page.record(
            duration_secs=4,
            selectors=[
                {"type": "css", "value": "header"},
                {"type": "css", "value": "main"},
            ],
            output_dir=str(OUTPUT_DIR / "regions"),
            write_frames=True,
        )
        for region in multi.regions:
            print(
                f"  region {region.label}: "
                f"bbox={region.bbox} frames={len(region.frames)}"
            )


async def _record_while_clicking() -> None:
    """Record an interaction rather than a page load.

    The page gets its own browser window, so the recording runs concurrently
    instead of pinning the tab to the foreground and blocking sibling captures.
    Note the ordering: a plain `new_page` lands in the most recently active
    window, so the recording window is created last.
    """
    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page_in_window(TARGET_URL)
        print(f"alone in its window: {await page.alone_in_window()}")

        handle = await page.start_recording(duration_secs=30, fps=15)
        await page.evaluate_js("window.scrollBy({top: 600, behavior: 'smooth'})")
        await asyncio.sleep(2)

        rec = await handle.stop()
        print(
            f"interaction: {rec.frames_captured} frames, "
            f"held capture lock: {rec.foregrounded}"
        )


def main() -> None:
    """Record qscrape.dev/l2/eshop as frames, regions, and an interaction."""
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    asyncio.run(_record_page())
    asyncio.run(_record_while_clicking())


if __name__ == "__main__":
    main()
