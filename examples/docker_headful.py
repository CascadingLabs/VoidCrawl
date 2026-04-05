"""Use voidcrawl with Chrome running headful inside Docker.

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
    - Chrome navigating to qscrape.dev/l2/news (Mountainhome Herald)
    - The Astro JS islands hydrating and content appearing
    - A screenshot being captured
    - Chrome navigating to a second qscrape.dev/l2 site
    - Everything happens in real time
"""

import asyncio

from voidcrawl import BrowserPool, PoolConfig


async def main() -> None:
    """Connect to Docker headful Chrome and demonstrate navigation."""
    # PoolConfig.from_docker(headful=True) selects the right ports (19222/19223)
    # and probes them so you get a clear error if the container isn't running.
    async with BrowserPool(PoolConfig.from_docker(headful=True)) as pool:
        # ── Basic navigation ─────────────────────────────────────────
        async with pool.acquire() as tab:
            # goto() combines navigate + wait_for_network_idle — needed for
            # Astro client:only islands that render content after page load.
            event = await tab.goto(
                "https://qscrape.dev/l2/news",
                timeout=30.0,
            )
            print(f"Wait event: {event}")

            title = await tab.title()
            html = await tab.content()
            print(f"Title: {title}")
            print(f"HTML: {len(html):,} chars")

            # ── DOM queries ──────────────────────────────────────────
            headlines = await tab.query_selector_all(".hn-feed-headline")
            print(f"Article headlines found: {len(headlines)}")
            for h in headlines[:5]:
                print(f"  - {h}")

            # ── Screenshot ───────────────────────────────────────────
            png_bytes = await tab.screenshot_png()
            import anyio  # noqa: PLC0415

            path = "/tmp/docker_headful_screenshot.png"
            await anyio.Path(path).write_bytes(png_bytes)
            print(f"Screenshot: {len(png_bytes):,} bytes -> {path}")

            # ── JavaScript evaluation ────────────────────────────────
            article_count = await tab.evaluate_js(
                "document.querySelectorAll('.hn-feed-item').length"
            )
            print(f"Article cards in DOM: {article_count}")

        # ── Parallel fetch ───────────────────────────────────────────
        print("\nParallel fetch (watch both tabs in VNC!)...")

        async def fetch(url: str) -> tuple[str, int]:
            async with pool.acquire() as tab:
                await tab.goto(url)
                t = await tab.title()
                length = len(await tab.content())
                return t or "(no title)", length
            raise AssertionError("unreachable")

        results = await asyncio.gather(
            fetch("https://qscrape.dev/l2/eshop"),
            fetch("https://qscrape.dev/l2/scoretap"),
        )
        for title, length in results:
            print(f"  {title}: {length:,} chars")

    print("\nDone! The Docker container is still running.")
    print("Connect VNC to localhost:5900 to see the Chrome windows.")
    stop_cmd = "docker compose -f docker/docker-compose.headful.yml --profile amd down"
    print(f"Stop with: {stop_cmd}")


if __name__ == "__main__":
    asyncio.run(main())
