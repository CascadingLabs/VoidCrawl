"""Integration tests for accessibility-tree access.

Covers get_full_ax_tree / query_ax_tree / click_by_role across the PyO3
boundary. Requires a built extension (``./build.sh``) and Chrome/Chromium;
skipped automatically when Chrome is unavailable.

Run with:
    uv run pytest tests/test_ax_tree.py -v
"""

from __future__ import annotations

import shutil
from typing import Any
from urllib.parse import quote

import pytest

from voidcrawl import BrowserConfig, BrowserSession

_chrome_available = shutil.which("google-chrome") or shutil.which("chromium")

pytestmark = pytest.mark.skipif(
    not _chrome_available, reason="Chrome/Chromium not found on PATH"
)


def data_url(html: str) -> str:
    """Inline HTML as a navigable data: URL — no test server needed."""
    return "data:text/html," + quote(html)


def ax_role(node: dict[str, Any]) -> str:
    role = node.get("role")
    return "" if role is None else str(role.get("value", ""))


def ax_name(node: dict[str, Any]) -> str:
    name = node.get("name")
    return "" if name is None else str(name.get("value", ""))


class TestAxTree:
    async def test_get_full_ax_tree_resolves_implicit_role_and_name(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(
                data_url("<main><button>Load more</button></main>")
            )
            nodes = await page.get_full_ax_tree()

            assert isinstance(nodes, list)
            assert nodes, "expected a non-empty AX tree"
            button = next((n for n in nodes if ax_role(n) == "button"), None)
            assert button is not None, "implicit role=button should be present"
            assert ax_name(button) == "Load more"
            await page.close()

    async def test_query_ax_tree_matches_and_misses(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(
                data_url("<button>Save</button><button>Cancel</button>")
            )

            hits = await page.query_ax_tree(role="button", name="Cancel")
            assert len(hits) == 1
            assert ax_name(hits[0]) == "Cancel"

            misses = await page.query_ax_tree(role="button", name="Nope")
            assert misses == []
            await page.close()

    async def test_click_by_role_fires_handler(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(
                data_url('<button onclick="window.__hit=true">Subscribe</button>')
            )

            await page.click_by_role("button", "Subscribe")
            hit = await page.evaluate_js("window.__hit === true")
            assert hit is True
            await page.close()

    async def test_click_by_role_raises_when_missing(self) -> None:
        async with BrowserSession(BrowserConfig()) as browser:
            page = await browser.new_page(data_url("<button>Only</button>"))

            with pytest.raises(Exception, match="Missing"):
                await page.click_by_role("button", "Missing")
            await page.close()
