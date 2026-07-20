"""Capture action-triggered JSON without page-world fetch/XHR hooks.

This example is self-contained: it starts a local HTTP server and needs no
external site or API credentials.
"""

import asyncio
import json
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from voidcrawl import BrowserSession


class DemoHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/":
            body = b"""<!doctype html>
                <button onclick="Promise.all([
                    fetch('/api/overview'), fetch('/api/backlinks')
                ])">Load reports</button>
            """
            content_type = "text/html"
        elif self.path == "/api/overview":
            body = json.dumps({"domains": 42, "links": 120}).encode()
            content_type = "application/json"
        elif self.path == "/api/backlinks":
            body = json.dumps({"items": ["one.example", "two.example"]}).encode()
            content_type = "application/json"
        else:
            self.send_error(404)
            return

        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        pass


@contextmanager
def demo_server() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), DemoHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        yield f"http://{host}:{port}/"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


async def main() -> None:
    with demo_server() as url:
        async with BrowserSession() as browser, browser.page(url) as page:
            async with page.expect_responses(
                {
                    "overview": "**/api/overview",
                    "backlinks": "**/api/backlinks",
                },
                timeout=10,
            ) as pending:
                await page.click_by_role("button", "Load reports")

            responses = await pending.value
            print("overview:", await responses["overview"].json())
            print("backlinks:", await responses["backlinks"].json())


if __name__ == "__main__":
    asyncio.run(main())
