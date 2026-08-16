"""Network request logging: capture and inspect all sub-resource requests.

Uses VoidCrawl's built-in InstallNetworkObserver and CollectNetworkRequests
actions, which wrap the browser's PerformanceObserver API.

The observer uses ``buffered: true`` so it retroactively captures all
resource entries from the current navigation — install it after the page
loads, then collect.
"""

import asyncio

from voidcrawl import BrowserConfig, BrowserPool, BrowserSession, PoolConfig
from voidcrawl.actions import (
    CollectNetworkRequests,
    Flow,
    InstallNetworkObserver,
)

TARGET_URL = "https://qscrape.dev/l2/news"


async def basic_logging() -> None:
    """Capture all network requests from a page load."""
    print("=== Basic network logging ===\n")

    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle()

        # Install observer after load — buffered: true picks up
        # all resources that already loaded during navigation.
        await InstallNetworkObserver().run(page)
        requests = await CollectNetworkRequests(clear=True).run(page)

        print(f"Captured {len(requests)} network requests:\n")
        for r in requests:
            print(
                f"  [{r['type']:>10}]"
                f"  {r['duration']:>4}ms"
                f"  {r['size']:>6}B"
                f"  {r['name']}"
            )

        await page.close()


async def pool_logging() -> None:
    """Capture network requests using a BrowserPool."""
    print("\n=== Pool network logging ===\n")

    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        resp = await tab.goto(TARGET_URL)
        print(f"Page: {resp.url} (status {resp.status_code})")

        await InstallNetworkObserver().run(tab)
        requests = await CollectNetworkRequests().run(tab)

        print(f"{len(requests)} requests captured")
        for r in requests:
            print(f"  {r['name']}")


async def flow_logging() -> None:
    """Use a Flow to compose observer install + collect."""
    print("\n=== Flow-based logging ===\n")

    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle()

        # Compose install + collect in a single Flow
        flow = Flow(
            [
                InstallNetworkObserver(),
                CollectNetworkRequests(clear=True),
            ]
        )
        result = await flow.run(page)
        requests = result.last

        print(f"Flow captured {len(requests)} requests")
        for r in requests:
            print(f"  {r['type']:>10}  {r['name']}")

        await page.close()


async def filtered_requests() -> None:
    """Filter captured requests by initiator type."""
    print("\n=== Filtered requests (script only) ===\n")

    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle()

        await InstallNetworkObserver().run(page)
        all_requests = await CollectNetworkRequests().run(page)

        scripts = [r for r in all_requests if r["type"] == "script"]

        print(f"Total requests: {len(all_requests)}")
        print(f"Script requests: {len(scripts)}")
        for r in scripts:
            print(f"  {r['name']}")

        await page.close()


async def main() -> None:
    await basic_logging()
    await pool_logging()
    await flow_logging()
    await filtered_requests()


if __name__ == "__main__":
    asyncio.run(main())
