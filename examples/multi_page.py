"""Work with multiple pages (tabs) in a single browser session — in parallel."""

import asyncio

from voidcrawl import BrowserConfig, BrowserSession, Page

# All four qscrape.dev Level 2 sites — each is a JS-rendered Astro island.
URLS = [
    "https://qscrape.dev/l2/news",
    "https://qscrape.dev/l2/eshop",
    "https://qscrape.dev/l2/scoretap",
    "https://qscrape.dev/l2/taxes",
]


async def _open(browser: BrowserSession, url: str) -> tuple[str, str]:
    """Open *url* in a new tab, wait for hydration, return (title, url)."""
    page: Page = await browser.new_page(url)
    await page.wait_for_network_idle()
    return (await page.title() or "(no title)", await page.url() or "")


async def main() -> None:
    """Open all four L2 sites in parallel tabs and print their titles."""
    async with BrowserSession(BrowserConfig()) as browser:
        results = await asyncio.gather(*[_open(browser, url) for url in URLS])

    for title, url in results:
        print(f"  {url}  ->  {title}")


if __name__ == "__main__":
    asyncio.run(main())
