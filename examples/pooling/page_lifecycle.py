"""Create, initialize, and clean up pages with first-class lifecycle APIs."""

import asyncio

from voidcrawl import BrowserSession


async def main() -> None:
    async with BrowserSession() as browser:
        # browser.page() always closes the tab, including when the body raises.
        async with browser.page() as page:
            await page.add_init_script("globalThis.__voidcrawlReady = 'yes'")
            await page.goto(
                "data:text/html,<title>Lifecycle demo</title><h1>Hello</h1>"
            )
            print("title:", await page.title())
            print("init script:", await page.evaluate_js("globalThis.__voidcrawlReady"))

        # Page creation no longer serializes by temporarily taking the session.
        pages = await asyncio.gather(*(browser.new_page() for _ in range(3)))
        print("concurrent blank pages:", len(pages))
        await asyncio.gather(*(page.close() for page in pages))


if __name__ == "__main__":
    asyncio.run(main())
