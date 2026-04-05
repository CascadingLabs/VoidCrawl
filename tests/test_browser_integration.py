"""Integration tests for cookie API and network observer actions.

Requires a built extension (``./build.sh``) and Chrome/Chromium installed.
Skipped automatically when Chrome is unavailable.

Run with:
    uv run pytest tests/test_browser_integration.py -v
"""

from __future__ import annotations

import shutil

import pytest

from voidcrawl import BrowserConfig, BrowserPool, BrowserSession, PoolConfig
from voidcrawl.actions import CollectNetworkRequests, InstallNetworkObserver

_chrome_available = shutil.which("google-chrome") or shutil.which("chromium")

pytestmark = pytest.mark.skipif(
    not _chrome_available, reason="Chrome/Chromium not found on PATH"
)


# ── Cookie tests (BrowserSession) ───────────────────────────────────────


class TestCookiesSession:
    @pytest.mark.asyncio
    async def test_set_and_get_cookies(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("https://example.com")

            await page.set_cookie("test_name", "test_value")
            cookies = await page.get_cookies()

            names = [c["name"] for c in cookies]
            assert "test_name" in names

            match = next(c for c in cookies if c["name"] == "test_name")
            assert match["value"] == "test_value"
            await page.close()

    @pytest.mark.asyncio
    async def test_set_cookie_with_options(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("https://example.com")

            await page.set_cookie(
                "secure_cookie",
                "s3cret",
                secure=True,
                http_only=True,
            )
            cookies = await page.get_cookies()
            match = next(c for c in cookies if c["name"] == "secure_cookie")
            assert match["value"] == "s3cret"
            assert match["secure"] is True
            assert match["httpOnly"] is True
            await page.close()

    @pytest.mark.asyncio
    async def test_delete_cookie(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("https://example.com")

            await page.set_cookie("to_delete", "val")
            cookies_before = await page.get_cookies()
            assert any(c["name"] == "to_delete" for c in cookies_before)

            await page.delete_cookie("to_delete")
            cookies_after = await page.get_cookies()
            assert not any(c["name"] == "to_delete" for c in cookies_after)
            await page.close()

    @pytest.mark.asyncio
    async def test_multiple_cookies(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("https://example.com")

            await page.set_cookie("c1", "v1")
            await page.set_cookie("c2", "v2")
            await page.set_cookie("c3", "v3")

            cookies = await page.get_cookies()
            names = {c["name"] for c in cookies}
            assert {"c1", "c2", "c3"} <= names
            await page.close()

    @pytest.mark.asyncio
    async def test_cookies_empty_on_fresh_page(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("about:blank")
            cookies = await page.get_cookies()
            assert cookies == []
            await page.close()


# ── Cookie tests (BrowserPool) ──────────────────────────────────────────


class TestCookiesPool:
    @pytest.mark.asyncio
    async def test_set_and_get_cookies_pooled(self) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            await tab.navigate("https://example.com")
            await tab.wait_for_navigation()

            await tab.set_cookie("pool_cookie", "pool_value")
            cookies = await tab.get_cookies()

            match = next(c for c in cookies if c["name"] == "pool_cookie")
            assert match["value"] == "pool_value"

    @pytest.mark.asyncio
    async def test_delete_cookie_pooled(self) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            await tab.navigate("https://example.com")
            await tab.wait_for_navigation()

            await tab.set_cookie("temp", "val")
            await tab.delete_cookie("temp")

            cookies = await tab.get_cookies()
            assert not any(c["name"] == "temp" for c in cookies)


# ── Network observer tests (BrowserSession) ─────────────────────────────


class TestNetworkObserverIntegration:
    @pytest.mark.asyncio
    async def test_observer_captures_requests(self) -> None:
        """Install after navigation; buffered: true picks up past entries."""
        async with BrowserSession(BrowserConfig()) as browser:
            # Use a JS-rendered page that loads many sub-resources
            page = await browser.new_page("https://qscrape.dev/l2/news")
            await page.wait_for_network_idle()

            await InstallNetworkObserver().run(page)
            requests = await CollectNetworkRequests().run(page)

            assert isinstance(requests, list)
            assert len(requests) > 0  # qscrape loads many JS/CSS
            await page.close()

    @pytest.mark.asyncio
    async def test_observer_clear_resets_log(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("https://qscrape.dev/l2/news")
            await page.wait_for_network_idle()

            await InstallNetworkObserver().run(page)

            # Collect with clear
            first = await CollectNetworkRequests(clear=True).run(page)
            assert isinstance(first, list)
            assert len(first) > 0

            # Second collect should be empty (log was cleared)
            second = await CollectNetworkRequests().run(page)
            assert second == []
            await page.close()

    @pytest.mark.asyncio
    async def test_observer_entries_have_expected_keys(self) -> None:
        """Verify that captured entries contain the expected fields."""
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page("https://qscrape.dev/l2/news")
            await page.wait_for_network_idle()

            await InstallNetworkObserver().run(page)
            result = await CollectNetworkRequests().run(page)
            assert isinstance(result, list)
            requests = result

            assert len(requests) > 0
            entry = requests[0]
            assert "name" in entry
            assert "type" in entry
            assert "duration" in entry
            assert "size" in entry
            await page.close()


# ── Network observer tests (BrowserPool) ────────────────────────────────


class TestNetworkObserverPool:
    @pytest.mark.asyncio
    async def test_observer_with_pool(self) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            resp = await tab.goto("https://qscrape.dev/l2/news")
            assert resp.status_code == 200

            await InstallNetworkObserver().run(tab)
            requests = await CollectNetworkRequests().run(tab)

            assert isinstance(requests, list)
            assert len(requests) > 0
