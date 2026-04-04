"""Use void_crawl with Chrome running headful inside Docker.

This example connects to Chrome instances running in the Docker headful
container (Sway + wayvnc + GPU-accelerated Chrome). You can watch
everything Chrome does by opening a VNC client to localhost:5900.

Setup (run this first in a separate terminal):

    ./docker/run-headful.sh          # auto-detects your GPU
    # or: ./docker/run-headful.sh --gpu amd

Then run this script:

    python examples/docker_headful.py

Watch Chrome live in your browser:
    Open http://localhost:6080 and click Connect.
    (Or use a VNC client on localhost:5900 for lower latency.)

What you'll see:
    - Chrome navigating to Wikipedia
    - The page fully rendering with images, CSS, layout
    - A screenshot being captured
    - Chrome navigating to a second URL
    - Everything happens in real time
"""

import asyncio

from void_crawl import BrowserConfig, BrowserPool, PoolConfig


async def main() -> None:
    """Connect to Docker headful Chrome and demonstrate navigation."""
    # ── Connect to Docker Chrome ─────────────────────────────────────
    # The headful Docker container runs Chrome on ports 19222 and 19223.
    config = PoolConfig(
        chrome_ws_urls=["http://localhost:19222", "http://localhost:19223"],
        tabs_per_browser=2,
        browser=BrowserConfig(headless=False),
    )

    async with BrowserPool(config) as pool:
        # ── Basic navigation ─────────────────────────────────────────
        async with pool.acquire() as tab:
            # goto() combines navigate + wait-for-network-idle in one shot.
            event = await tab.goto(
                "https://en.wikipedia.org/wiki/Web_scraping",
                timeout=30.0,
            )
            print(f"Wait event: {event}")

            title = await tab.title()
            html = await tab.content()
            print(f"Title: {title}")
            print(f"HTML: {len(html):,} chars")

            # ── DOM queries ──────────────────────────────────────────
            headings = await tab.query_selector_all("#toc li a")
            print(f"Table of contents entries: {len(headings)}")
            for h in headings[:5]:
                print(f"  - {h}")

            # ── Screenshot ───────────────────────────────────────────
            png_bytes = await tab.screenshot_png()
            import anyio  # noqa: PLC0415

            path = "/tmp/docker_headful_screenshot.png"
            await anyio.Path(path).write_bytes(png_bytes)
            print(f"Screenshot: {len(png_bytes):,} bytes -> {path}")

            # ── JavaScript evaluation ────────────────────────────────
            link_count = await tab.evaluate_js('document.querySelectorAll("a").length')
            print(f"Links on page: {link_count}")

        # ── Parallel fetch ───────────────────────────────────────────
        print("\nParallel fetch (watch both tabs in VNC!)...")

        async def fetch(url: str) -> tuple[str, int]:
            async with pool.acquire() as tab:
                await tab.goto(url)
                t = await tab.title()
                length = len(await tab.content())
                return t or "(no title)", length

        results = await asyncio.gather(
            fetch("https://en.wikipedia.org/wiki/Web_scraping"),
            fetch("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
        )
        for title, length in results:
            print(f"  {title}: {length:,} chars")

    print("\nDone! The Docker container is still running.")
    print("Connect VNC to localhost:5900 to see the Chrome windows.")
    stop_cmd = "docker compose -f docker/docker-compose.headful.yml --profile amd down"
    print(f"Stop with: {stop_cmd}")


if __name__ == "__main__":
    asyncio.run(main())
