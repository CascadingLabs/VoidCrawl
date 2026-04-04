"""Rust-native CDP browser automation for Python via PyO3.

``void_crawl`` provides async-first browser automation backed by a Rust
CDP (Chrome DevTools Protocol) core, exposed to Python through PyO3.
Launch headless or headful Chrome sessions, manage pooled tabs for
concurrent crawling, and compose reusable browser actions.

Example:
    Minimal single-page scrape::

        async with BrowserSession() as browser:
            page = await browser.new_page("https://example.com")
            html = await page.content()

    Pooled concurrent crawling::

        async with BrowserPool(PoolConfig(browsers=2, tabs_per_browser=4)) as pool:
            async with pool.acquire() as tab:
                await tab.navigate("https://example.com")
"""

from __future__ import annotations

import os

from pydantic import BaseModel, Field

from void_crawl._ext import (
    BrowserPool as _BrowserPool,
)
from void_crawl._ext import (
    BrowserSession as _BrowserSession,
)
from void_crawl._ext import (
    Page,
    PooledTab,
    _AcquireContext,
    _PoolParamsContext,
)

__all__ = [
    "BrowserConfig",
    "BrowserPool",
    "BrowserSession",
    "Page",
    "PoolConfig",
    "PooledTab",
]


# ── Configuration models ────────────────────────────────────────────────


class BrowserConfig(BaseModel):
    """Configuration for launching or connecting to a single browser instance.

    Controls headless/headful mode, stealth patches, proxy routing, and
    custom Chrome flags.  Pass an instance to :class:`BrowserSession` or
    embed one inside :class:`PoolConfig`.

    Attributes:
        headless: Run Chrome without a visible window. Defaults to ``True``.
        stealth: Apply anti-detection patches (navigator overrides, etc.).
            Defaults to ``True``.
        no_sandbox: Disable the Chrome sandbox. Required in some Docker
            environments. Defaults to ``False``.
        proxy: Upstream HTTPS proxy URL, e.g. ``"http://proxy:8080"``.
        chrome_executable: Path to a custom Chrome/Chromium binary.
            When ``None``, the bundled Chromium discovery is used.
        extra_args: Additional command-line flags forwarded to Chrome.
        ws_url: Connect to an **already-running** Chrome instance via its
            WebSocket debugger URL instead of launching a new one.

    Example:
        >>> cfg = BrowserConfig(headless=False, stealth=True)
        >>> async with BrowserSession(cfg) as browser:
        ...     page = await browser.new_page("https://example.com")
    """

    headless: bool = True
    stealth: bool = True
    no_sandbox: bool = False
    proxy: str | None = None
    chrome_executable: str | None = None
    extra_args: list[str] = Field(default_factory=list)
    ws_url: str | None = None


class PoolConfig(BaseModel):
    """Configuration for a pool of reusable browser tabs.

    Controls how many Chrome processes to launch, how many concurrent tabs
    each process may hold, and when tabs are recycled or evicted.

    Attributes:
        browsers: Number of Chrome processes in the pool. Defaults to ``1``.
        tabs_per_browser: Maximum concurrent tabs **per** Chrome process.
            Defaults to ``4``.
        tab_max_uses: Hard-recycle a tab after this many navigations.
            Prevents memory leaks in long-running crawls. Defaults to ``50``.
        tab_max_idle_secs: Evict a tab that has been idle longer than this
            many seconds. Defaults to ``60``.
        chrome_ws_urls: Pre-existing Chrome WebSocket debugger URLs.  When
            non-empty, the pool connects to these instead of launching
            new processes, and *browsers* is ignored.
        browser: Shared :class:`BrowserConfig` applied to every Chrome
            process launched by the pool.

    Example:
        >>> cfg = PoolConfig(browsers=2, tabs_per_browser=4)
        >>> async with BrowserPool(cfg) as pool:
        ...     async with pool.acquire() as tab:
        ...         await tab.navigate("https://example.com")

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
        """Build a :class:`PoolConfig` from environment variables.

        Reads the following variables (all optional):

        +------------------------+---------------------------------+---------+
        | Variable               | Description                     | Default |
        +========================+=================================+=========+
        | ``CHROME_WS_URLS``     | Comma-separated ws/http URLs    | —       |
        +------------------------+---------------------------------+---------+
        | ``BROWSER_COUNT``      | Chrome processes to launch      | 1       |
        +------------------------+---------------------------------+---------+
        | ``TABS_PER_BROWSER``   | Max concurrent tabs per browser | 4       |
        +------------------------+---------------------------------+---------+
        | ``TAB_MAX_USES``       | Hard recycle threshold          | 50      |
        +------------------------+---------------------------------+---------+
        | ``TAB_MAX_IDLE_SECS``  | Idle eviction timeout           | 60      |
        +------------------------+---------------------------------+---------+
        | ``CHROME_NO_SANDBOX``  | Set to ``"1"`` to disable       | —       |
        +------------------------+---------------------------------+---------+
        | ``CHROME_HEADLESS``    | Set to ``"0"`` for headful      | 1       |
        +------------------------+---------------------------------+---------+

        Returns:
            A fully-populated :class:`PoolConfig`.

        Example:
            >>> cfg = PoolConfig.from_env()
            >>> async with BrowserPool(cfg) as pool:
            ...     async with pool.acquire() as tab:
            ...         await tab.navigate("https://example.com")
        """
        ws_urls_raw = os.environ.get("CHROME_WS_URLS", "")
        chrome_ws_urls = [u.strip() for u in ws_urls_raw.split(",") if u.strip()]

        browser_count = (
            int(os.environ.get("BROWSER_COUNT", "1"))
            if not chrome_ws_urls
            else len(chrome_ws_urls)
        )

        return cls(
            browsers=browser_count,
            tabs_per_browser=int(os.environ.get("TABS_PER_BROWSER", "4")),
            tab_max_uses=int(os.environ.get("TAB_MAX_USES", "50")),
            tab_max_idle_secs=int(os.environ.get("TAB_MAX_IDLE_SECS", "60")),
            chrome_ws_urls=chrome_ws_urls,
            browser=BrowserConfig(
                no_sandbox=os.environ.get("CHROME_NO_SANDBOX") == "1",
                headless=os.environ.get("CHROME_HEADLESS", "1") != "0",
            ),
        )


# ── BrowserSession ──────────────────────────────────────────────────────


class BrowserSession:
    """Async context manager wrapping a single Chromium instance via CDP.

    Use as an ``async with`` block.  On entry the browser is launched (or
    connected to, if :attr:`BrowserConfig.ws_url` is set); on exit the
    process is terminated and resources are freed.

    Args:
        config: Browser launch options.  Defaults to
            ``BrowserConfig()`` (headless + stealth).

    Example:
        >>> async with BrowserSession(BrowserConfig(headless=False)) as browser:
        ...     page = await browser.new_page("https://example.com")
        ...     html = await page.content()
    """

    def __init__(self, config: BrowserConfig | None = None) -> None:
        self._config = config if config is not None else BrowserConfig()
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
        """Open a new tab and navigate to *url*.

        Args:
            url: The URL to load in the new tab.

        Returns:
            The new tab handle.
        """
        assert self._inner is not None, "BrowserSession not started — use async with"
        return await self._inner.new_page(url)

    async def version(self) -> str:
        """Return the browser version string (e.g. ``"Chrome/126.0.6478.126"``).

        Returns:
            The Chrome/Chromium product version reported by the browser.
        """
        assert self._inner is not None, "BrowserSession not started — use async with"
        return await self._inner.version()

    async def close(self) -> None:
        """Shut down the browser process immediately.

        Called automatically on ``__aexit__``; only needed if you want
        to close the browser without leaving the ``async with`` block.
        """
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
    """Pool of reusable browser tabs across one or more Chrome processes.

    Manages a semaphore-bounded set of recycled tabs.  Tabs are navigated
    to ``about:blank`` on release rather than closed, making subsequent
    acquires near-instant.  Tabs are hard-recycled after
    :attr:`PoolConfig.tab_max_uses` navigations and evicted after
    :attr:`PoolConfig.tab_max_idle_secs` of inactivity.

    Args:
        config: Pool sizing and browser launch options.

    Example:
        >>> cfg = PoolConfig(browsers=2, tabs_per_browser=4)
        >>> async with BrowserPool(cfg) as pool:
        ...     async with pool.acquire() as tab:
        ...         await tab.navigate("https://example.com")
        ...         html = await tab.content()

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
        """Check out a tab from the pool as an async context manager.

        The tab is automatically returned to the pool when the context
        exits, even on exception.

        Returns:
            An async context manager yielding a :class:`PooledTab`.

        Example:
            >>> async with pool.acquire() as tab:
            ...     await tab.navigate("https://example.com")
        """
        assert self._inner is not None, "BrowserPool not started — use async with"
        return self._inner.acquire()

    async def warmup(self) -> None:
        """Pre-open tabs across all browser sessions.

        Call after entering the pool context to eliminate cold-start
        latency on the first :meth:`acquire` calls.
        """
        assert self._inner is not None, "BrowserPool not started — use async with"
        await self._inner.warmup()

    def __repr__(self) -> str:
        cfg = self._config
        return (
            f"BrowserPool(browsers={cfg.browsers},"
            f" tabs_per_browser={cfg.tabs_per_browser})"
        )
