"""Cookie management: set, read, and delete cookies via CDP.

Demonstrates VoidCrawl's cookie API which uses Chrome's CDP cookie
layer — supports HttpOnly and Secure flags unlike document.cookie.
"""

import asyncio

from voidcrawl import BrowserConfig, BrowserPool, BrowserSession, PoolConfig


async def session_cookies() -> None:
    """Set, read, and delete cookies using BrowserSession."""
    print("=== BrowserSession cookies ===\n")

    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page("https://example.com")

        # Set cookies (CDP-level, supports HttpOnly/Secure)
        await page.set_cookie("session_id", "abc123")
        await page.set_cookie(
            "auth_token",
            "s3cret",
            secure=True,
            http_only=True,
        )
        await page.set_cookie("locale", "en-US", path="/")

        # Read all cookies for the current page
        cookies = await page.get_cookies()
        print(f"Cookies set: {len(cookies)}")
        for c in cookies:
            flags = []
            if c.get("secure"):
                flags.append("Secure")
            if c.get("httpOnly"):
                flags.append("HttpOnly")
            flag_str = f" [{', '.join(flags)}]" if flags else ""
            print(f"  {c['name']}={c['value']}{flag_str}")

        # Delete one cookie
        await page.delete_cookie("locale")
        remaining = await page.get_cookies()
        names = [c["name"] for c in remaining]
        print(f"\nAfter deleting 'locale': {names}")

        await page.close()


async def pool_cookies() -> None:
    """Set cookies on a pooled tab and navigate with them."""
    print("\n=== BrowserPool cookies ===\n")

    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        # Navigate to the domain first to scope cookies
        await tab.navigate("https://example.com")
        await tab.wait_for_navigation()

        # Set an authentication cookie
        await tab.set_cookie(
            "api_key",
            "my-key-123",
            secure=True,
            http_only=True,
        )

        # Navigate to a different path — cookie persists
        resp = await tab.goto("https://example.com/")
        cookies = await tab.get_cookies()

        print(f"Page: {resp.url}")
        print(f"Status: {resp.status_code}")
        print(f"Cookies: {[c['name'] for c in cookies]}")


async def main() -> None:
    await session_cookies()
    await pool_cookies()


if __name__ == "__main__":
    asyncio.run(main())
