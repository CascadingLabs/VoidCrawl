"""Type stubs for the voidcrawl top-level package."""

from __future__ import annotations

from voidcrawl._ext import (
    Page as Page,
)
from voidcrawl._ext import (
    PageResponse as PageResponse,
)
from voidcrawl._ext import (
    PooledTab as PooledTab,
)
from voidcrawl._ext import (
    _AcquireContext as _AcquireContext,
)
from voidcrawl.actions._protocol import (
    JsTab as JsTab,
)
from voidcrawl.actions._protocol import (
    Tab as Tab,
)
from voidcrawl.scale import (
    ScaleProfile as ScaleProfile,
)
from voidcrawl.scale import (
    ScaleReport as ScaleReport,
)
from voidcrawl.schema import (
    Attr as Attr,
)
from voidcrawl.schema import (
    Schema as Schema,
)
from voidcrawl.schema import (
    Text as Text,
)
from voidcrawl.schema import (
    safe_url as safe_url,
)
from voidcrawl.schema import (
    strip_tags as strip_tags,
)

Selector = Text

__all__ = [
    "Attr",
    "BrowserConfig",
    "BrowserPool",
    "BrowserSession",
    "JsTab",
    "Page",
    "PageResponse",
    "PoolConfig",
    "PooledTab",
    "ScaleProfile",
    "ScaleReport",
    "Schema",
    "Selector",
    "Tab",
    "Text",
    "safe_url",
    "strip_tags",
]

class BrowserConfig:
    """Configuration for launching or connecting to a single browser instance."""

    headless: bool
    stealth: bool
    no_sandbox: bool
    proxy: str | None
    chrome_executable: str | None
    extra_args: list[str]
    ws_url: str | None
    debug: bool
    stepping: bool
    highlight: bool
    step_delay: float

    def __init__(
        self,
        *,
        headless: bool = True,
        stealth: bool = True,
        no_sandbox: bool = False,
        proxy: str | None = None,
        chrome_executable: str | None = None,
        extra_args: list[str] = ...,
        ws_url: str | None = None,
        debug: bool = False,
        stepping: bool = True,
        highlight: bool = True,
        step_delay: float = 0.3,
    ) -> None: ...
    def model_dump(self) -> dict[str, object]: ...

class PoolConfig:
    """Configuration for a pool of reusable browser tabs."""

    browsers: int
    tabs_per_browser: int
    tab_max_uses: int
    tab_max_idle_secs: int
    acquire_timeout_secs: int
    auto_evict: bool
    chrome_ws_urls: list[str]
    browser: BrowserConfig

    def __init__(
        self,
        *,
        browsers: int = 1,
        tabs_per_browser: int = 4,
        tab_max_uses: int = 50,
        tab_max_idle_secs: int = 60,
        acquire_timeout_secs: int = 30,
        auto_evict: bool = True,
        chrome_ws_urls: list[str] = ...,
        browser: BrowserConfig = ...,
    ) -> None: ...
    @classmethod
    def from_profile(
        cls,
        profile: ScaleProfile = "balanced",
        *,
        env: str = "auto",
    ) -> PoolConfig: ...
    @classmethod
    def from_docker(
        cls,
        *,
        headful: bool = False,
        host: str = "localhost",
        ports: list[int] | None = None,
        tabs_per_browser: int = 4,
        check: bool = True,
    ) -> PoolConfig: ...
    @classmethod
    def from_env(cls) -> PoolConfig: ...
    def model_dump(self) -> dict[str, object]: ...

class BrowserSession:
    """Async context manager wrapping a single Chromium instance via CDP."""

    _config: BrowserConfig
    def __init__(self, config: BrowserConfig | None = None) -> None: ...
    async def __aenter__(self) -> BrowserSession: ...
    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> bool: ...
    async def new_page(self, url: str) -> Page: ...
    async def version(self) -> str: ...
    async def close(self) -> None: ...

class BrowserPool:
    """Pool of reusable browser tabs across one or more Chrome processes."""

    def __init__(self, config: PoolConfig) -> None: ...
    async def __aenter__(self) -> BrowserPool: ...
    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> bool: ...
    def acquire(self) -> _AcquireContext: ...
    async def warmup(self) -> None: ...
