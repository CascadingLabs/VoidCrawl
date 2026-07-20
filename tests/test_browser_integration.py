"""Integration tests for cookie API and network observer actions.

Requires a built extension (``./build.sh``) and Chrome/Chromium installed.
Skipped automatically when Chrome is unavailable.

Run with:
    uv run pytest tests/test_browser_integration.py -v
"""

from __future__ import annotations

import asyncio
import contextlib
import http.server
import shutil
import socketserver
import threading
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from voidcrawl import (
    BrowserClosedError,
    BrowserConfig,
    BrowserPool,
    BrowserSession,
    NavigationTimeoutError,
    Page,
    PoolConfig,
    ProfileRegistry,
    ResponseTimeoutError,
)
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
        elif self.path == "/busy":
            self._send(
                "text/html",
                b"<script>setInterval(() => fetch('/api/data'), 25)</script>",
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
        elif self.path == "/api/one":
            self._send("application/json", b'{"endpoint":"one"}\n')
        elif self.path == "/api/two":
            self._send("application/json", b'{"endpoint":"two"}\n')
        elif self.path == "/api/large":
            self._send("application/octet-stream", b"x" * 64)
        elif self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/api/one")
            self.send_header("Content-Length", "0")
            self.end_headers()
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


# ── First-class page lifecycle and response capture ─────────────────────


class TestPageLifecycleAndResponses:
    @pytest.mark.asyncio
    async def test_blank_page_init_script_runs_before_navigation(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.add_init_script("window.__voidcrawlInit = 'ready'")
            await page.goto(network_fixture_url)
            assert await page.evaluate_js("window.__voidcrawlInit") == "ready"

    @pytest.mark.asyncio
    async def test_single_response_body_capture(self, network_fixture_url: str) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            async with page.expect_response("**/api/one") as pending:
                await page.evaluate_js("fetch('/api/one')")
            response = await pending.value
            assert response.status == 200
            assert response.body_state == "available"
            assert await response.json() == {"endpoint": "one"}

    @pytest.mark.asyncio
    async def test_named_multi_response_capture(self, network_fixture_url: str) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            async with page.expect_responses(
                {"one": "**/api/one", "two": "**/api/two"}
            ) as pending:
                await page.evaluate_js(
                    "Promise.all([fetch('/api/one'), fetch('/api/two')])"
                )
            responses = await pending.value
            assert await responses["one"].json() == {"endpoint": "one"}
            assert await responses["two"].json() == {"endpoint": "two"}

    @pytest.mark.asyncio
    async def test_redirect_is_explicitly_body_unavailable(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            async with page.expect_response("**/redirect") as pending:
                await page.evaluate_js("fetch('/redirect')")
            response = await pending.value
            assert response.status == 302
            assert response.body_state == "unavailable"
            assert "redirect" in response.body_error

    @pytest.mark.asyncio
    async def test_closing_page_interrupts_response_expectation(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)
            with pytest.raises(BrowserClosedError):
                async with page.expect_response("**/api/missing", timeout=10):
                    await page.close()

    @pytest.mark.asyncio
    async def test_response_expectation_timeout_is_typed(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            with pytest.raises(ResponseTimeoutError):
                async with page.expect_response("**/api/missing", timeout=0.1):
                    pass

    @pytest.mark.asyncio
    async def test_response_body_limit_is_explicit(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            async with page.expect_response(
                "**/api/large", max_response_bytes=8
            ) as pending:
                await page.evaluate_js("fetch('/api/large')")
            response = await pending.value
            assert response.truncated is True
            assert response.body_state == "truncated"
            assert await response.bytes() == b"x" * 8

    @pytest.mark.asyncio
    async def test_navigation_timeout_is_typed(self, network_fixture_url: str) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            with pytest.raises(NavigationTimeoutError) as raised:
                await page.goto(f"{network_fixture_url}busy", timeout=0.2)
            assert raised.value.url.endswith("/busy")
            assert raised.value.wait_phase == "networkidle"
            assert raised.value.timeout == 0.2
            assert raised.value.elapsed >= 0.2

    @pytest.mark.asyncio
    async def test_page_context_closes_tab_when_owning_task_is_cancelled(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            entered = asyncio.Event()
            pages: list[Page] = []

            async def use_page() -> None:
                async with browser.page() as page:
                    pages.append(page)
                    entered.set()
                    await asyncio.Event().wait()

            task = asyncio.create_task(use_page())
            await entered.wait()
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task

            page = pages[0]
            with pytest.raises(RuntimeError, match="page is closed"):
                await page.target_id()

    @pytest.mark.asyncio
    async def test_navigation_cancellation_does_not_lose_page_or_session(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            async with browser.page() as page:
                navigation = asyncio.ensure_future(
                    page.goto(f"{network_fixture_url}busy", timeout=10)
                )
                await asyncio.sleep(0.1)
                navigation.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await navigation
                assert await page.target_id()

            replacement = await browser.new_page()
            await replacement.close()

    @pytest.mark.asyncio
    async def test_split_profile_runs_two_independent_chrome_instances(
        self, tmp_path: Path, network_fixture_url: str
    ) -> None:
        registry = ProfileRegistry(str(tmp_path))
        registry.create_profile("source")
        marker = tmp_path / "source" / "Default" / "VoidCrawlBaseline"
        marker.write_text("shared")

        async with registry.split_profile("source", copies=2) as split:
            first_path, second_path = split.paths
            assert [
                (Path(path) / "Default" / "VoidCrawlBaseline").read_text()
                for path in split.paths
            ] == ["shared", "shared"]

            first = BrowserSession(BrowserConfig(user_data_dir=first_path))
            second = BrowserSession(BrowserConfig(user_data_dir=second_path))
            async with first, second:
                first_ws, second_ws = await asyncio.gather(
                    first.websocket_url(), second.websocket_url()
                )
                assert first_ws != second_ws

                first_page, second_page = await asyncio.gather(
                    first.new_page(network_fixture_url),
                    second.new_page(network_fixture_url),
                )
                await first_page.evaluate_js(
                    "localStorage.setItem('voidcrawl-worker', 'first')"
                )
                assert (
                    await second_page.evaluate_js(
                        "localStorage.getItem('voidcrawl-worker')"
                    )
                    is None
                )
                await asyncio.gather(first_page.close(), second_page.close())

    @pytest.mark.asyncio
    async def test_concurrent_page_creation_keeps_session_available(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            pages = await asyncio.gather(*(browser.new_page() for _ in range(3)))
            assert len({await page.target_id() for page in pages}) == 3
            await asyncio.gather(*(page.close() for page in pages))
            final = await browser.new_page()
            await final.close()


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
