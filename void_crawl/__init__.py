"""void_crawl — Rust-native CDP browser automation for Python."""

from __future__ import annotations

import os

from pydantic import BaseModel, Field

from void_crawl._ext import (
    BrowserSession as _BrowserSession,
    BrowserPool as _BrowserPool,
    Page,
    PooledTab,
    _AcquireContext,
    _PoolParamsContext,
)

__all__ = [
    "BrowserConfig",
    "PoolConfig",
    "BrowserSession",
    "BrowserPool",
    "Page",
    "PooledTab",
]


# ── Configuration models ────────────────────────────────────────────────


class BrowserConfig(BaseModel):
    """Configuration for a single browser instance.

    Example::

        cfg = BrowserConfig(headless=False, stealth=True)
        async with BrowserSession(cfg) as browser:
            page = await browser.new_page("https://example.com")
    """

    headless: bool = True
    stealth: bool = True
    no_sandbox: bool = False
    proxy: str | None = None
    chrome_executable: str | None = None
    extra_args: list[str] = Field(default_factory=list)
    ws_url: str | None = None


class PoolConfig(BaseModel):
    """Configuration for a browser pool.

    Example::

        cfg = PoolConfig(browsers=2, tabs_per_browser=4)
        async with BrowserPool(cfg) as pool:
            async with pool.acquire() as tab:
                await tab.navigate("https://example.com")

    Load from environment variables::

        cfg = PoolConfig.from_env()
    """

    browsers: int = 1
    tabs_per_browser: int = 4
    tab_max_uses: int = 50
    tab_max_idle_secs: int = 60
    chrome_ws_urls: list[str] = Field(default_factory=list)
    browser: BrowserConfig = Field(default_factory=BrowserConfig)

    @classmethod
    def from_env(cls) -> PoolConfig:
        """Build a PoolConfig from environment variables.

        | Variable            | Description                                    | Default |
        |---------------------|------------------------------------------------|---------|
        | ``CHROME_WS_URLS``  | Comma-separated ws:// or http:// URLs          | —       |
        | ``BROWSER_COUNT``   | Number of Chrome processes to launch           | 1       |
        | ``TABS_PER_BROWSER``| Max concurrent tabs per browser                | 4       |
        | ``TAB_MAX_USES``    | Hard recycle threshold                         | 50      |
        | ``TAB_MAX_IDLE_SECS``| Idle eviction timeout                         | 60      |
        | ``CHROME_NO_SANDBOX``| Set to "1" to disable sandbox                 | —       |
        | ``CHROME_HEADLESS`` | Set to "0" for headful mode                    | 1       |
        """
        ws_urls_raw = os.environ.get("CHROME_WS_URLS", "")
        chrome_ws_urls = [u.strip() for u in ws_urls_raw.split(",") if u.strip()]

        browser_count = (
            int(os.environ.get("BROWSER_COUNT", 1))
            if not chrome_ws_urls
            else len(chrome_ws_urls)
        )

        return cls(
            browsers=browser_count,
            tabs_per_browser=int(os.environ.get("TABS_PER_BROWSER", 4)),
            tab_max_uses=int(os.environ.get("TAB_MAX_USES", 50)),
            tab_max_idle_secs=int(os.environ.get("TAB_MAX_IDLE_SECS", 60)),
            chrome_ws_urls=chrome_ws_urls,
            browser=BrowserConfig(
                no_sandbox=os.environ.get("CHROME_NO_SANDBOX") == "1",
                headless=os.environ.get("CHROME_HEADLESS", "1") != "0",
            ),
        )


# ── BrowserSession ──────────────────────────────────────────────────────


class BrowserSession:
    """Browser session wrapping a Chromium instance via CDP.

    Example::

        async with BrowserSession(BrowserConfig(headless=False)) as browser:
            page = await browser.new_page("https://example.com")
            html = await page.content()
    """

    def __init__(self, config: BrowserConfig = BrowserConfig()) -> None:
        self._config = config
        self._inner: _BrowserSession | None = None

    async def __aenter__(self) -> BrowserSession:
        bc = self._config
        inner = _BrowserSession(
            headless=bc.headless,
            stealth=bc.stealth,
            no_sandbox=bc.no_sandbox,
            proxy=bc.proxy,
            chrome_executable=bc.chrome_executable,
            extra_args=bc.extra_args,
            ws_url=bc.ws_url,
        )
        self._inner = await inner.__aenter__()
        return self

    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> bool:
        if self._inner is not None:
            return await self._inner.__aexit__(exc_type, exc_val, exc_tb)
        return False

    async def new_page(self, url: str) -> Page:
        """Open a new tab and navigate to url."""
        assert self._inner is not None, "BrowserSession not started — use async with"
        return await self._inner.new_page(url)

    async def version(self) -> str:
        """Return the browser version string."""
        assert self._inner is not None, "BrowserSession not started — use async with"
        return await self._inner.version()

    async def close(self) -> None:
        """Close the browser."""
        if self._inner is not None:
            await self._inner.close()

    def __repr__(self) -> str:
        mode = (
            "ws"
            if self._config.ws_url
            else ("headless" if self._config.headless else "headful")
        )
        return f"BrowserSession(mode={mode})"


# ── BrowserPool ─────────────────────────────────────────────────────────


class BrowserPool:
    """Pool of reusable browser tabs across one or more Chrome sessions.

    Example::

        cfg = PoolConfig(browsers=2, tabs_per_browser=4)
        async with BrowserPool(cfg) as pool:
            async with pool.acquire() as tab:
                await tab.navigate("https://example.com")
                html = await tab.content()

    Load config from environment::

        async with BrowserPool(PoolConfig.from_env()) as pool:
            ...
    """

    def __init__(self, config: PoolConfig) -> None:
        self._config = config
        self._inner: _BrowserPool | None = None

    async def __aenter__(self) -> BrowserPool:
        cfg = self._config
        bc = cfg.browser
        ctx: _PoolParamsContext = _BrowserPool._from_params(
            browsers=cfg.browsers,
            tabs_per_browser=cfg.tabs_per_browser,
            tab_max_uses=cfg.tab_max_uses,
            tab_max_idle_secs=cfg.tab_max_idle_secs,
            headless=bc.headless,
            no_sandbox=bc.no_sandbox,
            stealth=bc.stealth,
            ws_urls=cfg.chrome_ws_urls,
            proxy=bc.proxy,
            chrome_executable=bc.chrome_executable,
            extra_args=bc.extra_args,
        )
        self._inner = await ctx.__aenter__()
        return self

    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> bool:
        if self._inner is not None:
            return await self._inner.__aexit__(exc_type, exc_val, exc_tb)
        return False

    def acquire(self) -> _AcquireContext:
        """Return a context manager that checks out a tab from the pool.

        Example::

            async with pool.acquire() as tab:
                await tab.navigate("https://example.com")
        """
        assert self._inner is not None, "BrowserPool not started — use async with"
        return self._inner.acquire()

    async def warmup(self) -> None:
        """Pre-open tabs across all sessions for faster first acquires."""
        assert self._inner is not None, "BrowserPool not started — use async with"
        await self._inner.warmup()

    def __repr__(self) -> str:
        cfg = self._config
        return f"BrowserPool(browsers={cfg.browsers}, tabs_per_browser={cfg.tabs_per_browser})"
