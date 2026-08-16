"""Query DOM elements, type into search inputs, and click buttons.

Uses qscrape.dev/l2/taxes (Eldoria Registry of Deeds) — a JS-rendered
property-search form that only exists in the DOM after hydration.
"""

import asyncio

from voidcrawl import BrowserConfig, BrowserSession

TARGET_URL = "https://qscrape.dev/l2/taxes"


async def main() -> None:
    """Search the Eldoria Registry: type a name, click search, read results."""
    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page(TARGET_URL)
        # Wait for Astro client:only islands to hydrate before touching the DOM.
        await page.wait_for_network_idle()

        # Query all search inputs (returns inner-HTML strings or None per element)
        inputs = await page.query_selector_all(".er-input")
        print(f"Search fields available: {len(inputs)}")

        # Type a search term into the first input field
        await page.type_into(".er-input", "Smith")

        # Click the primary search / submit button via JS (avoids CDP viewport
        # requirement that page.click_element() uses under the hood).
        await page.evaluate_js("document.querySelector('.er-btn-primary').click()")

        # Results may load via a client-side fetch — wait again.
        await page.wait_for_network_idle()

        # Query result rows
        rows = await page.query_selector_all(".er-row")
        print(f"Result rows after search: {len(rows)}")

        # Peek at the first result row's inner HTML
        first = await page.query_selector(".er-row")
        print(f"First row: {(first or '').strip()[:120]}")

        # Missing selectors return None
        missing = await page.query_selector("#does-not-exist")
        print(f"Missing element: {missing}")


if __name__ == "__main__":
    asyncio.run(main())
