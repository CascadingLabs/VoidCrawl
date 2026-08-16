"""Evaluate arbitrary JavaScript in a JS-rendered page context."""

import asyncio

from voidcrawl import BrowserPool, PoolConfig

TARGET_URL = "https://qscrape.dev/l2/scoretap"


async def main() -> None:
    """Evaluate various JavaScript expressions on ScoreTap after hydration."""
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        # goto() waits for network idle so JS islands have fully hydrated.
        await tab.goto(TARGET_URL)

        # evaluate_js returns native Python types
        user_agent = await tab.evaluate_js("navigator.userAgent")
        print(f"User agent: {user_agent}")

        # Count JS-rendered match rows (0 in raw HTML — only visible after hydration)
        match_count = await tab.evaluate_js(
            "document.querySelectorAll('.st-match-row').length"
        )
        print(f"Number of match rows: {match_count}")

        # Return structured data
        dims = await tab.evaluate_js("({w: window.innerWidth, h: window.innerHeight})")
        print(f"Viewport: {dims}")

        # Modify the DOM via JS
        await tab.evaluate_js("document.title = 'Modified by voidcrawl'")
        print(f"New title: {await tab.title()}")


if __name__ == "__main__":
    asyncio.run(main())
