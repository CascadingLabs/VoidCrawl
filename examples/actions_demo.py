"""Quick scrape of qscrape.dev/l2/news (Mountainhome Herald feed).

Extracts all article cards from the feed and prints them as structured data.
"""

import asyncio
import json

import voidcrawl as vc
from voidcrawl import BrowserConfig, BrowserSession
from voidcrawl.actions import QueryAll


class Article(vc.Schema):
    headline: str = vc.Selector(".hn-feed-headline")
    category: str | None = vc.Selector(".hn-feed-cat")
    excerpt: str | None = vc.Selector(".hn-feed-excerpt", sanitize=vc.strip_tags)
    meta: str | None = vc.Selector(".hn-feed-meta")
    href: str | None = vc.Attr("a", "href", sanitize=vc.safe_url)
    img: str | None = vc.Attr(".hn-feed-img", "src", sanitize=vc.safe_url)


TARGET_URL = "https://qscrape.dev/l2/news"


async def main() -> None:
    async with BrowserSession(BrowserConfig(headless=True)) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle()
        articles: list[Article] = await QueryAll(".hn-feed-item", Article).run(page)

        # --- Printing the output ---

        print(f"Found {len(articles)} article(s):\n")
        for i, a in enumerate(articles, 1):
            print(f"[{i}] {a.headline}")
            if a.category:
                print(f"     Category : {a.category}")
            if a.meta:
                print(f"     Meta     : {a.meta}")
            if a.excerpt:
                print(f"     Excerpt  : {a.excerpt}")
            if a.href:
                print(f"     Link     : {a.href}")
            print()

        print("--- raw JSON ---")
        print(json.dumps([a.model_dump() for a in articles], indent=2))


if __name__ == "__main__":
    asyncio.run(main())
