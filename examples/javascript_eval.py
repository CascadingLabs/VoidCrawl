"""Evaluate arbitrary JavaScript in the page context."""

import asyncio

from void_crawl import BrowserPool, PoolConfig


async def main() -> None:
    """Evaluate various JavaScript expressions in a page context."""
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        await tab.navigate("https://example.com")

        # evaluate_js returns native Python types
        user_agent = await tab.evaluate_js("navigator.userAgent")
        print(f"User agent: {user_agent}")

        # Compute something in-page
        p_count = await tab.evaluate_js("document.querySelectorAll('p').length")
        print(f"Number of <p> tags: {p_count}")

        # Return structured data
        dims = await tab.evaluate_js("({w: window.innerWidth, h: window.innerHeight})")
        print(f"Viewport: {dims}")

        # Modify the DOM via JS
        await tab.evaluate_js("document.title = 'Modified by void_crawl'")
        print(f"New title: {await tab.title()}")


if __name__ == "__main__":
    asyncio.run(main())
