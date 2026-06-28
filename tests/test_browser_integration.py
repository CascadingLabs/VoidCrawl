"""Integration tests for cookie API and network observer actions.

Requires a built extension (``./build.sh``) and Chrome/Chromium installed.
Skipped automatically when Chrome is unavailable.

Run with:
    uv run pytest tests/test_browser_integration.py -v
"""

from __future__ import annotations

import contextlib
import http.server
import shutil
import socketserver
import threading
import urllib.parse
from typing import TYPE_CHECKING

import pytest

from voidcrawl import BrowserConfig, BrowserPool, BrowserSession, PoolConfig
from voidcrawl.actions import CollectNetworkRequests, InstallNetworkObserver

if TYPE_CHECKING:
    from collections.abc import Iterator

_chrome_available = shutil.which("google-chrome") or shutil.which("chromium")

pytestmark = pytest.mark.skipif(
    not _chrome_available, reason="Chrome/Chromium not found on PATH"
)


class _NetworkFixtureHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/":
            self._send(
                "text/html",
                b"""<!doctype html>
<html>
  <head>
    <title>VoidCrawl network fixture</title>
    <link rel="stylesheet" href="/style.css">
  </head>
  <body>
    <main>network fixture</main>
    <script src="/app.js"></script>
  </body>
</html>
""",
            )
        elif self.path == "/style.css":
            self._send("text/css", b"main { color: rgb(10 20 30); }\n")
        elif self.path == "/app.js":
            self._send(
                "application/javascript",
                (
                    b"fetch('/api/data')"
                    b".then(r => r.json())"
                    b".then(d => { window.fixtureData = d; });\n"
                ),
            )
        elif self.path == "/api/data":
            self._send("application/json", b'{"ok":true,"source":"voidcrawl-test"}\n')
        else:
            self.send_error(404)

    def log_message(self, fmt: str, *args: object) -> None:
        return

    def _send(self, content_type: str, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


@pytest.fixture
def network_fixture_url(unused_tcp_port: int) -> Iterator[str]:
    server = socketserver.ThreadingTCPServer(
        ("127.0.0.1", unused_tcp_port),
        _NetworkFixtureHandler,
    )
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    try:
        yield f"http://127.0.0.1:{unused_tcp_port}/"
    finally:
        server.shutdown()
        server.server_close()
        with contextlib.suppress(RuntimeError):
            thread.join(timeout=2)


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


# ── Lazy CDP escalation tests (BrowserSession) ──────────────────────────


class TestLazyCdpIntegration:
    @pytest.mark.asyncio
    async def test_public_js_apis_have_expected_minimal_cdp_transitions(
        self,
    ) -> None:
        frame_html = (
            "<html><body><script>"
            "setTimeout(() => {"
            "const el = document.createElement('div');"
            "el.id = 'late';"
            "document.body.appendChild(el);"
            "}, 25);"
            "</script>"
            '<iframe srcdoc="'
            "<script>window.voidcrawlFrameValue=7</script><p>frame</p>"
            '"></iframe></body></html>'
        )
        url = "data:text/html," + urllib.parse.quote(frame_html)

        async with BrowserSession(BrowserConfig(no_sandbox=True)) as browser:
            page = await browser.new_page(url)
            before = await page.instrumentation_state()
            ready_state = await page.eval_js("document.readyState")
            after_eval = await page.instrumentation_state()

            await page.wait_for_selector("#late", timeout=2.0)
            after_wait = await page.instrumentation_state()

            result = await page.evaluate_js_in_frame(
                "about:srcdoc",
                "window.voidcrawlFrameValue",
            )
            after = await page.instrumentation_state()

            assert ready_state in {"interactive", "complete"}
            assert result == 7
            assert before.low_cdp is True
            assert before.runtime_enabled is False
            assert before.network_enabled is False
            assert after_eval.low_cdp is True
            assert after_eval.runtime_enabled is False
            assert after_eval.network_enabled is False
            assert after_wait.low_cdp is True
            assert after_wait.runtime_enabled is False
            assert after_wait.network_enabled is False
            assert after.low_cdp is False
            assert after.runtime_enabled is True
            assert after.network_enabled is False
            await page.close()


# ── Network observer tests (BrowserSession) ─────────────────────────────


class TestNetworkObserverIntegration:
    @pytest.mark.asyncio
    async def test_observer_captures_requests(self, network_fixture_url: str) -> None:
        """Install after navigation; buffered: true picks up past entries."""
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)
            await page.wait_for_network_idle()

            await InstallNetworkObserver().run(page)
            requests = await CollectNetworkRequests().run(page)

            assert isinstance(requests, list)
            names = {entry["name"] for entry in requests}
            assert f"{network_fixture_url}style.css" in names
            assert f"{network_fixture_url}app.js" in names
            assert f"{network_fixture_url}api/data" in names
            await page.close()

    @pytest.mark.asyncio
    async def test_observer_clear_resets_log(self, network_fixture_url: str) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)
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
    async def test_observer_entries_have_expected_keys(
        self,
        network_fixture_url: str,
    ) -> None:
        """Verify that captured entries contain the expected fields."""
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)
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
    async def test_observer_with_pool(self, network_fixture_url: str) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            resp = await tab.goto(network_fixture_url)
            # `status_code` is documented as None when the response is served
            # from disk cache / a service worker or otherwise not captured —
            # which a recycled pool tab with a warm cache hits intermittently.
            # This test verifies the observer works with a pooled tab (like its
            # sibling Session tests, which assert no status); accept the
            # documented None, but still fail on a real error status.
            assert resp.status_code in (200, None)

            await InstallNetworkObserver().run(tab)
            requests = await CollectNetworkRequests().run(tab)

            assert isinstance(requests, list)
            assert len(requests) > 0
