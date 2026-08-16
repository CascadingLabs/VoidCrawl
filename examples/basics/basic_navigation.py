"""Basic navigation: launch a browser, visit a page, read its content."""

import asyncio

from voidcrawl import BrowserPool, PoolConfig

TARGET_URL = "https://qscrape.dev/l2/news"


async def main() -> None:
    """Launch a headless browser, visit the Mountainhome Herald, and print page info."""
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        # goto() combines navigate + wait_for_network_idle in one call —
        # required for JS-rendered pages like qscrape.dev/l2/*.
        await tab.goto(TARGET_URL)

        title = await tab.title()
        url = await tab.url()
        html = await tab.content()

        print(f"Title: {title}")
        print(f"URL:   {url}")
        print(f"HTML length: {len(html)} chars")


if __name__ == "__main__":
    asyncio.run(main())
