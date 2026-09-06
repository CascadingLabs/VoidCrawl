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
import urllib.parse
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from voidcrawl import (
    BrowserClosedError,
    BrowserConfig,
    BrowserPool,
    BrowserSession,
    InterruptRequest,
    NavigationTimeoutError,
    Page,
    PoolConfig,
    ProfileRegistry,
    ResponseTimeoutError,
    SessionInterrupted,
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
        elif self.path == "/action":
            self._send(
                "text/html",
                b"""<!doctype html>
<button type="button">Load data</button>
<script>
document.querySelector('button').addEventListener('click', () => fetch('/api/one'));
</script>
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
    async def test_bounded_dom_and_accessibility_snapshots(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            dom = await page.rendered_dom_snapshot(max_bytes=128)
            ax = await page.accessibility_snapshot(max_nodes=1)

        assert dom.state == "truncated"
        assert dom.retained_bytes == 128
        assert len(dom.bytes()) == 128
        assert dom.epoch is not None
        assert dom.byte_report["domain"] == "rendered_dom_utf8"
        assert dom.byte_report["extent"]["status"] == "truncated"
        assert dom.byte_report["accounting"]["retained"] == 128
        assert "network fixture" not in repr(dom)

        assert ax.state == "truncated"
        assert ax.nodes_retained == 1
        assert ax.nodes_observed >= 1
        assert ax.frame_scope == "top_level"
        assert ax.byte_report["domain"] == "accessibility_json_utf8"
        assert ax.byte_report["accounting"]["retained"] == ax.retained_bytes
        assert "network fixture" not in repr(ax)

    @pytest.mark.asyncio
    async def test_visual_and_layout_snapshots_include_capture_metadata(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            layout = await page.layout_snapshot()
            visual = await page.visual_snapshot(full_page=False)

        assert layout.epoch is not None
        assert layout.layout_viewport[2] > 0
        assert layout.content_size[2] > 0
        assert layout.device_scale_factor is not None
        assert visual.epoch == layout.epoch
        assert visual.region == "viewport"
        assert visual.format == "png"
        assert visual.image_size[0] > 0
        assert visual.complete is True
        assert visual.byte_report["domain"] == "screenshot_png"
        assert visual.byte_report["spec"] is None
        assert visual.byte_report["extent"]["status"] == "complete"
        assert visual.retained_bytes == len(visual.bytes())
        assert visual.bytes().startswith(b"\x89PNG")
        assert "network fixture" not in repr(visual)

    @pytest.mark.asyncio
    async def test_isolated_context_disposal_removes_origin_state(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            assert await browser.state_binding() == "shared_browser_profile"
            isolated = await browser.new_isolated_context()
            page = isolated.page()
            assert await page.state_binding() == "isolated_browser_context"
            await page.goto(network_fixture_url)
            await page.evaluate_js(
                "document.cookie='isolated=secret; path=/'; "
                "localStorage.setItem('isolated', 'secret')"
            )
            report = await isolated.dispose()
            assert report.cleanup_complete is True
            assert report.disposal_state == "disposed"

            clean = await browser.new_isolated_context()
            clean_page = clean.page()
            await clean_page.goto(network_fixture_url)
            state = await clean_page.evaluate_js(
                "({cookie: document.cookie, local: localStorage.getItem('isolated')})"
            )
            assert state == {"cookie": "", "local": None}
            assert (await clean.dispose()).cleanup_complete is True

    @pytest.mark.asyncio
    async def test_isolated_context_manager_disposes_on_error(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            context = await browser.new_isolated_context()

            async def failed_capture() -> None:
                async with context:
                    page = context.page()
                    await page.goto(network_fixture_url)
                    await page.evaluate_js("localStorage.setItem('leak', 'no')")
                    raise RuntimeError("capture failed")

            with pytest.raises(RuntimeError, match="capture failed"):
                await failed_capture()

            clean = await browser.new_isolated_context()
            page = clean.page()
            await page.goto(network_fixture_url)
            assert await page.evaluate_js("localStorage.getItem('leak')") is None
            assert (await clean.dispose()).cleanup_complete is True

    @pytest.mark.asyncio
    async def test_navigation_capture_returns_source_and_safe_resource_graph(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserSession(BrowserConfig()) as browser, browser.page() as page:
            capture = await page.arm_navigation_capture(max_duration=5.0)
            await page.navigate(network_fixture_url)
            await asyncio.sleep(0.1)
            report = await capture.finish()

        assert report.termination == "finished"
        assert report.source_status == 200
        assert report.source_body_state == "available"
        source_body = report.source_body()
        assert source_body is not None
        assert b"VoidCrawl network fixture" in source_body
        assert report.resource_count >= 3
        assert report.cleanup_complete is True
        assert report.network_extra_info == "unavailable_in_current_client"
        source_byte_report = report.source_byte_report
        assert source_byte_report is not None
        assert source_byte_report["domain"] == "cdp_decoded_body"
        assert source_byte_report["extent"]["status"] == "complete"
        assert all("url" not in resource for resource in report.resources())
        assert any(
            "url" in resource for resource in report.resources(include_urls=True)
        )
        assert "VoidCrawl network fixture" not in repr(report)

    @pytest.mark.asyncio
    async def test_observation_scope_captures_prearmed_lifecycle_markers(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser, browser.page() as page:
            scope = await page.arm_observation(
                collect_network=False,
                collect_console=True,
                collect_exceptions=False,
                max_events=8,
                max_diagnostic_bytes=8,
                max_duration=5.0,
            )
            await page.evaluate_js("console.log('observation-marker')")
            await asyncio.sleep(0.01)
            report = await scope.finish()
            with pytest.raises(ValueError, match="at least one"):
                await page.arm_observation(
                    collect_network=False,
                    collect_console=False,
                    collect_exceptions=False,
                )

        assert report["termination"] == "finished"
        assert report["cleanup_complete"] is True
        assert any(event["kind"] == "console_api_called" for event in report["events"])
        assert report["diagnostic_bytes_retained"] == 8
        assert report["diagnostic_bytes_dropped"] > 0
        assert report["diagnostics"][0]["truncated"] is True
        assert report["byte_report"]["domain"] == "runtime_diagnostic_utf8"
        assert report["byte_report"]["spec"]["budget_scope"] == "capture_aggregate"
        assert "text" not in report["diagnostics"][0]
        assert report["accounting"]["bytes"]["retained"] == {
            "status": "known",
            "value": 8,
        }

    @pytest.mark.asyncio
    async def test_environment_snapshot_reports_effective_provider_facts(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            snapshot = await page.environment_snapshot()

        assert snapshot["controller"]["name"] == "void_crawl_core"
        assert snapshot["renderer"]["product"]
        assert snapshot["mode"] == {"status": "known", "value": "headless"}
        assert snapshot["rendering"]["viewport"]["status"] == "known"
        assert snapshot["rendering"]["device_scale_factor"]["status"] == "known"
        assert snapshot["rendering"]["user_agent"]["status"] == "known"
        assert snapshot["capabilities"]["rendered_dom"] == {"status": "supported"}

        serialized = repr(snapshot)
        for forbidden in (
            "user_data_dir",
            "profile_path",
            "ws_url",
            "request_headers",
            "cookie_values",
        ):
            assert forbidden not in serialized

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
    async def test_explicit_interrupt_parks_same_target_and_rejects_mutation(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page(network_fixture_url) as page,
        ):
            target_id = await page.target_id()
            await page.evaluate_js("sessionStorage.setItem('interrupt-proof', 'kept')")
            interrupt = await browser.interrupt(
                page,
                InterruptRequest(
                    code="policy.operator_review", summary="fixture review"
                ),
            )

            assert interrupt.target_id == target_id
            assert "network fixture" in await page.content()
            with pytest.raises(SessionInterrupted) as exc:
                await page.evaluate_js("document.body.dataset.mutated = 'yes'")
            assert exc.value.interrupt_id == interrupt.interrupt_id

            resumed = await browser.resume(interrupt.interrupt_id)
            assert resumed.state == "resumed"
            assert await page.target_id() == target_id
            assert (
                await page.evaluate_js("sessionStorage.getItem('interrupt-proof')")
                == "kept"
            )

    @pytest.mark.asyncio
    async def test_interrupt_rejects_page_from_another_session(
        self, network_fixture_url: str
    ) -> None:
        async with (
            BrowserSession(BrowserConfig()) as first,
            BrowserSession(BrowserConfig()) as second,
        ):
            page = await first.new_page(network_fixture_url)
            with pytest.raises(RuntimeError, match="does not belong"):
                await second.interrupt(
                    page,
                    InterruptRequest(
                        code="policy.operator_review", summary="wrong owner"
                    ),
                )

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
            assert str(raised.value) == "navigation timed out"
            assert raised.value.code == "voidcrawl.navigation.timeout"
            assert raised.value.category == "timeout"
            assert raised.value.url.endswith("/busy")
            assert raised.value.wait_phase == "networkidle"
            assert raised.value.timeout == 0.2
            assert raised.value.elapsed >= 0.2

    @pytest.mark.asyncio
    async def test_pooled_tab_captures_action_response(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            await tab.goto(f"{network_fixture_url}action")
            async with tab.expect_response("**/api/one") as pending:
                await tab.click_by_role("button", "Load data")

            response = await pending.value
            assert response.status == 200
            assert await response.json() == {"endpoint": "one"}

    @pytest.mark.asyncio
    async def test_pooled_tab_response_options_fail_closed(self) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            with pytest.raises(ValueError, match="timeout"):
                tab.expect_response("**/api/one", timeout=0)
            with pytest.raises(ValueError, match="byte limits"):
                tab.expect_response("**/api/one", max_response_bytes=0)
            with pytest.raises(ValueError, match="must not be empty"):
                tab.expect_responses({})

    @pytest.mark.asyncio
    async def test_pooled_tab_response_expectation_rejects_released_tab(self) -> None:
        async with BrowserPool(PoolConfig()) as pool:
            async with pool.acquire() as tab:
                pending = tab.expect_response("**/api/one", timeout=0.1)

            with pytest.raises(RuntimeError, match="tab has been released"):
                async with pending:
                    pass

    @pytest.mark.asyncio
    async def test_pooled_tab_cannot_release_while_expectation_is_active(
        self, network_fixture_url: str
    ) -> None:
        async with BrowserPool(PoolConfig()) as pool:
            checkout = pool.acquire()
            tab = await checkout.__aenter__()
            await tab.goto(f"{network_fixture_url}action")
            pending = tab.expect_response("**/api/one")
            await pending.__aenter__()

            with pytest.raises(RuntimeError, match="active response expectation"):
                await checkout.__aexit__(None, None, None)

            await tab.click_by_role("button", "Load data")
            await pending.__aexit__(None, None, None)
            response = await pending.value
            assert response.status == 200

            await checkout.__aexit__(None, None, None)

    @pytest.mark.asyncio
    async def test_repeated_error_exit_does_not_poison_pooled_tab_release(self) -> None:
        async with BrowserPool(PoolConfig()) as pool:
            checkout = pool.acquire()
            tab = await checkout.__aenter__()
            pending = tab.expect_response("**/api/missing", timeout=0.1)
            await pending.__aenter__()

            await pending.__aexit__(RuntimeError, None, None)
            await pending.__aexit__(RuntimeError, None, None)
            await checkout.__aexit__(None, None, None)

    @pytest.mark.asyncio
    async def test_concurrent_duplicate_enter_and_exit_cannot_deadlock_or_leak(
        self,
    ) -> None:
        async with BrowserPool(PoolConfig()) as pool:
            checkout = pool.acquire()
            tab = await checkout.__aenter__()
            pending = tab.expect_response("**/api/missing", timeout=0.1)
            await pending.__aenter__()

            duplicate, _ = await asyncio.wait_for(
                asyncio.gather(
                    pending.__aenter__(),
                    pending.__aexit__(RuntimeError, None, None),
                    return_exceptions=True,
                ),
                timeout=2,
            )
            if not isinstance(duplicate, BaseException):
                await pending.__aexit__(RuntimeError, None, None)

            await checkout.__aexit__(None, None, None)

    @pytest.mark.asyncio
    async def test_cancelled_response_exit_does_not_poison_pooled_tab_release(
        self,
    ) -> None:
        async with BrowserPool(PoolConfig()) as pool:
            checkout = pool.acquire()
            tab = await checkout.__aenter__()
            pending = tab.expect_response("**/api/missing", timeout=30)
            await pending.__aenter__()

            exit_task = asyncio.ensure_future(pending.__aexit__(None, None, None))
            await asyncio.sleep(0.05)
            exit_task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await exit_task

            await checkout.__aexit__(None, None, None)

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
    async def test_set_and_get_cookies(self, network_fixture_url: str) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)

            await page.set_cookie("test_name", "test_value")
            cookies = await page.get_cookies()

            names = [c["name"] for c in cookies]
            assert "test_name" in names

            match = next(c for c in cookies if c["name"] == "test_name")
            assert match["value"] == "test_value"
            await page.close()

    @pytest.mark.asyncio
    async def test_set_cookie_with_options(self, network_fixture_url: str) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)

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
    async def test_delete_cookie(self, network_fixture_url: str) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)

            await page.set_cookie("to_delete", "val")
            cookies_before = await page.get_cookies()
            assert any(c["name"] == "to_delete" for c in cookies_before)

            await page.delete_cookie("to_delete")
            cookies_after = await page.get_cookies()
            assert not any(c["name"] == "to_delete" for c in cookies_after)
            await page.close()

    @pytest.mark.asyncio
    async def test_multiple_cookies(self, network_fixture_url: str) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(network_fixture_url)

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
    async def test_set_and_get_cookies_pooled(self, network_fixture_url: str) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            await tab.navigate(network_fixture_url)
            await tab.wait_for_navigation()

            await tab.set_cookie("pool_cookie", "pool_value")
            cookies = await tab.get_cookies()

            match = next(c for c in cookies if c["name"] == "pool_cookie")
            assert match["value"] == "pool_value"

    @pytest.mark.asyncio
    async def test_delete_cookie_pooled(self, network_fixture_url: str) -> None:
        async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
            await tab.navigate(network_fixture_url)
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
        async def run_smoke() -> None:
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

        try:
            await asyncio.wait_for(run_smoke(), timeout=45.0)
        except TimeoutError as exc:
            raise AssertionError("lazy-CDP browser smoke timed out after 45s") from exc


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
