"""Work with multiple pages (tabs) in a single browser session."""

import asyncio

from void_crawl import BrowserSession

URLS = [
    "https://example.com",
    "https://httpbin.org/html",
    "https://www.iana.org/domains/reserved",
]


async def main() -> None:
    """Open multiple tabs in one session and print their titles."""
    async with BrowserSession(headless=True) as browser:
        pages = [await browser.new_page(url) for url in URLS]
        for page in pages:
            title = await page.title()
            url = await page.url()
            print(f"  {url}  ->  {title}")
        for page in pages:
            await page.close()


if __name__ == "__main__":
    asyncio.run(main())
