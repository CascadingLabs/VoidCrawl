"""Basic navigation: launch a browser, visit a page, read its content."""

import asyncio

from void_crawl import BrowserPool


async def main() -> None:
    """Launch a headless browser, visit example.com, and print page info."""
    async with await BrowserPool.from_env() as pool:
        async with await pool.acquire() as tab:
            await tab.navigate("https://example.com")
            title = await tab.title()
            url = await tab.url()
            html = await tab.content()

            print(f"Title: {title}")
            print(f"URL:   {url}")
            print(f"HTML length: {len(html)} chars")


if __name__ == "__main__":
    asyncio.run(main())
