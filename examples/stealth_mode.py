"""Demonstrate stealth vs non-stealth browser sessions.

Stealth mode (enabled by default) patches common bot-detection signals:
  - navigator.webdriver is removed
  - navigator.plugins is populated
  - navigator.languages is set realistically
  - window.chrome.runtime is spoofed
  - navigator.permissions.query behaves like a real browser
"""

import asyncio
import json

from void_crawl import BrowserSession, Page

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
    fingerprint = json.loads(raw)
    print(f"\n[{label}]")
    for key, value in fingerprint.items():
        print(f"  {key}: {value}")


async def main() -> None:
    """Compare fingerprints with stealth enabled vs disabled."""
    # Stealth ON (default)
    async with BrowserSession(headless=True, stealth=True) as browser:
        page = await browser.new_page("https://example.com")
        await check_fingerprint("stealth=True", page)
        await page.close()

    # Stealth OFF
    async with BrowserSession(headless=True, stealth=False) as browser:
        page = await browser.new_page("https://example.com")
        await check_fingerprint("stealth=False", page)
        await page.close()


if __name__ == "__main__":
    asyncio.run(main())
