"""Set custom HTTP headers and scrape a JS-rendered page."""

import asyncio

import voidcrawl as vc
from voidcrawl import BrowserConfig, BrowserSession
from voidcrawl.actions import QueryAll

TARGET_URL = "https://qscrape.dev/l2/news"


class Article(vc.Schema):
    headline: str = vc.Selector(".hn-feed-headline")
    category: str | None = vc.Selector(".hn-feed-cat")


async def custom_headers() -> None:
    """Inject custom HTTP headers then scrape the Mountainhome Herald."""
    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page("about:blank")
        await page.set_headers(
            {
                "Accept-Language": "ja-JP,ja;q=0.9",
                "X-Custom-Token": "my-secret-token",
            }
        )

        # Navigate with headers set — they are included in every subsequent request.
        await page.navigate(TARGET_URL)
        await page.wait_for_network_idle()

        articles: list[Article] = await QueryAll(".hn-feed-item", Article).run(page)
        print(f"Loaded {len(articles)} article(s) with custom headers applied:\n")
        for a in articles[:5]:
            tag = f"[{a.category}] " if a.category else ""
            print(f"  {tag}{a.headline}")


async def with_proxy() -> None:
    """Launch a browser that routes traffic through a proxy.

    Requires a running proxy (e.g. `mitmproxy` on port 8080).
    Uncomment and adjust the proxy URL to try it out.
    """
    # proxy_cfg = BrowserConfig(proxy='http://127.0.0.1:8080')
    # async with BrowserSession(proxy_cfg) as browser:
    #     page = await browser.new_page(TARGET_URL)
    #     await page.wait_for_network_idle()
    #     print(await page.title())
    print("Proxy example is commented out — set a real proxy URL to run it.")


async def main() -> None:
    """Run the custom headers and proxy demos."""
    await custom_headers()
    await with_proxy()


if __name__ == "__main__":
    asyncio.run(main())
