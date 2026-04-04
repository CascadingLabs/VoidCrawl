"""Demonstrate stealth vs non-stealth browser sessions.

Stealth mode (enabled by default) patches common bot-detection signals:
  - navigator.webdriver is removed
  - navigator.plugins is populated
  - navigator.languages is set realistically
  - window.chrome.runtime is spoofed
  - navigator.permissions.query behaves like a real browser

Uses qscrape.dev/l2/scoretap — a JS-rendered site that requires a real
browser to render content, making it a realistic stealth test target.
"""

import asyncio
import json

from voidcrawl import BrowserConfig, BrowserSession, Page

TARGET_URL = "https://qscrape.dev/l2/scoretap"

DETECTION_JS = """
JSON.stringify({
    webdriver: navigator.webdriver,
    plugins_count: navigator.plugins.length,
    languages: navigator.languages,
    has_chrome_runtime: typeof window.chrome !== 'undefined'
        && typeof window.chrome.runtime !== 'undefined',
})
"""


async def check_fingerprint(label: str, page: Page) -> None:
    """Print bot-detection fingerprint signals for the given page."""
    raw = await page.evaluate_js(DETECTION_JS)
    fingerprint = json.loads(str(raw))
    print(f"\n[{label}]")
    for key, value in fingerprint.items():
        print(f"  {key}: {value}")


async def main() -> None:
    """Compare fingerprints with stealth enabled vs disabled."""
    # Stealth ON (default) — navigator.webdriver should be undefined/false
    async with BrowserSession(BrowserConfig(stealth=True)) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle()
        await check_fingerprint("stealth=True", page)

    # Stealth OFF — navigator.webdriver will be true, plugins empty
    async with BrowserSession(BrowserConfig(stealth=False)) as browser:
        page = await browser.new_page(TARGET_URL)
        await page.wait_for_network_idle()
        await check_fingerprint("stealth=False", page)


if __name__ == "__main__":
    asyncio.run(main())
