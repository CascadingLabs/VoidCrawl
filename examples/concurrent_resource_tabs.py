"""Extract four live resources as soon as each browser tab settles.

The QScrape targets are public test sites, so this example makes real network
requests without requiring credentials. Replace ``RESOURCE_URLS`` with the four
Ahrefs resource URLs used by your application.
"""

import asyncio
import time
from typing import NamedTuple, cast

from voidcrawl import BrowserSession

RESOURCE_URLS = {
    "news": "https://qscrape.dev/l2/news",
    "catalog": "https://qscrape.dev/l2/eshop",
    "scores": "https://qscrape.dev/l2/scoretap",
    "property": "https://qscrape.dev/l2/taxes",
}


class ResourceResult(NamedTuple):
    resource: str
    title: str | None
    text_chars: int
    html_bytes: int


async def extract_resource(
    browser: BrowserSession,
    resource: str,
    url: str,
) -> ResourceResult:
    # Each task owns one tab. This tab can settle and extract independently
    # while the other three tabs continue loading.
    async with browser.page() as page:
        await page.goto(url, wait_until="networkidle", timeout=60)
        title = await page.title()
        text_chars = cast(
            "int", await page.evaluate_js("document.body.innerText.length")
        )
        html = await page.content()
        return ResourceResult(resource, title, text_chars, len(html.encode()))


async def main() -> None:
    async with BrowserSession() as browser:
        started = time.monotonic()
        tasks = [
            asyncio.create_task(extract_resource(browser, resource, url))
            for resource, url in RESOURCE_URLS.items()
        ]

        # Do not put asyncio.gather() here: as_completed() lets downstream
        # processing begin as soon as any individual resource is ready.
        for completed in asyncio.as_completed(tasks):
            result = await completed
            elapsed = time.monotonic() - started
            print(
                f"{elapsed:0.2f}s {result.resource:8} "
                f"title={result.title!r} text={result.text_chars:,} chars "
                f"html={result.html_bytes:,} bytes"
            )


if __name__ == "__main__":
    asyncio.run(main())
