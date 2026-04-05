"""Type stubs for the voidcrawl._ext native extension module.

Internal — import from ``voidcrawl`` instead.
"""

from __future__ import annotations

from typing import Any

class PageResponse:
    """Result of :meth:`Page.goto` / :meth:`PooledTab.goto`.

    Attributes:
        html: Full outer HTML after network idle.
        url: Final URL after any redirects.
        status_code: HTTP status of the last response, or ``None``
            when served from cache / service worker.
        redirected: ``True`` when at least one HTTP redirect occurred.
    """

    html: str
    url: str
    status_code: int | None
    redirected: bool

class PooledTab:
    """A tab checked out from a :class:`~voidcrawl.BrowserPool`.

    Exposes the same page-interaction methods as :class:`Page` but must
    not be closed manually — return it to the pool via the async context
    manager or :meth:`~voidcrawl.BrowserPool.release`.

    Attributes:
        use_count: How many times this tab has been acquired (0 on first use).
    """

    use_count: int

    async def goto(self, url: str, timeout: float = 30.0) -> PageResponse:
        """Navigate to *url* and wait for network idle in one shot.

        Args:
            url: The URL to load.
            timeout: Maximum seconds to wait for network idle.

        Returns:
            A :class:`PageResponse` with HTML, final URL, status code,
            and redirect flag.
        """
        ...
    async def navigate(self, url: str) -> None:
        """Navigate to *url* without waiting for any load event.

        Args:
            url: The URL to load.
        """
        ...
    async def wait_for_navigation(self) -> None:
        """Block until the current navigation completes."""
        ...
    async def content(self) -> str:
        """Return the full page HTML (``document.documentElement.outerHTML``)."""
        ...
    async def title(self) -> str | None:
        """Return the document title, or ``None``."""
        ...
    async def url(self) -> str | None:
        """Return the current page URL, or ``None``."""
        ...
    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* and return the result.

        Args:
            expression: JavaScript expression or IIFE string.
        """
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes."""
        ...
    async def query_selector(self, selector: str) -> str | None:
        """Return the inner HTML of the first element matching *selector*, or ``None``.

        Args:
            selector: CSS selector string.
        """
        ...
    async def query_selector_all(self, selector: str) -> list[str]:
        """Return the inner HTML of every element matching *selector*.

        Args:
            selector: CSS selector string.
        """
        ...
    async def click_element(self, selector: str) -> None:
        """Click the first element matching *selector*.

        Args:
            selector: CSS selector string.
        """
        ...
    async def type_into(self, selector: str, text: str) -> None:
        """Focus the first element matching *selector* and type *text*.

        Args:
            selector: CSS selector string.
            text: The text to type.
        """
        ...
    async def set_headers(self, headers: dict[str, str]) -> None:
        """Set extra HTTP headers for all subsequent requests from this tab.

        Args:
            headers: Header name-value pairs.
        """
        ...
    async def get_cookies(self) -> list[dict[str, Any]]:
        """Return all cookies matching the current page URL.

        Each cookie is a dict with keys: ``name``, ``value``, ``domain``,
        ``path``, ``expires``, ``size``, ``httpOnly``, ``secure``, ``session``, etc.
        """
        ...
    async def set_cookie(
        self,
        name: str,
        value: str,
        *,
        domain: str | None = None,
        path: str | None = None,
        secure: bool | None = None,
        http_only: bool | None = None,
    ) -> None:
        """Set a cookie on the current page.

        Args:
            name: Cookie name.
            value: Cookie value.
            domain: Cookie domain (default: current page domain).
            path: Cookie path.
            secure: Mark as Secure.
            http_only: Mark as HttpOnly.
        """
        ...
    async def delete_cookie(
        self,
        name: str,
        *,
        domain: str | None = None,
        path: str | None = None,
    ) -> None:
        """Delete a cookie by name, optionally scoped to a domain and path.

        Args:
            name: Cookie name.
            domain: Cookie domain.
            path: Cookie path.
        """
        ...
    async def wait_for_stable_dom(
        self, timeout: float = 10.0, min_length: int = 5000, stable_checks: int = 5
    ) -> bool:
        """Wait until the DOM stabilises (stops changing).

        Polls the HTML length repeatedly and resolves once it stays
        constant across *stable_checks* consecutive checks.

        Args:
            timeout: Maximum seconds to wait.
            min_length: Minimum HTML length before checking stability.
            stable_checks: Consecutive unchanged polls required.

        Returns:
            ``True`` if the DOM stabilised, ``False`` on timeout.
        """
        ...
    async def wait_for_network_idle(self, timeout: float = 30.0) -> str | None:
        """Wait for network activity to settle.

        Args:
            timeout: Maximum seconds to wait.

        Returns:
            ``"networkIdle"`` or ``"networkAlmostIdle"`` on success,
            ``None`` on timeout.
        """
        ...
    async def dispatch_mouse_event(
        self,
        event_type: str,
        x: float,
        y: float,
        button: str = "left",
        click_count: int = 1,
        delta_x: float | None = None,
        delta_y: float | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchMouseEvent``.

        Args:
            event_type: One of ``"mousePressed"``, ``"mouseReleased"``,
                ``"mouseMoved"``, or ``"mouseWheel"``.
            x: Horizontal page coordinate.
            y: Vertical page coordinate.
            button: ``"left"``, ``"right"``, or ``"middle"``.
            click_count: Number of clicks (usually ``1``).
            delta_x: Horizontal scroll delta (``mouseWheel`` only).
            delta_y: Vertical scroll delta (``mouseWheel`` only).
            modifiers: Bit field for modifier keys (Ctrl=1, Shift=2, etc.).
        """
        ...
    async def dispatch_key_event(
        self,
        event_type: str,
        key: str | None = None,
        code: str | None = None,
        text: str | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchKeyEvent``.

        Args:
            event_type: ``"keyDown"``, ``"keyUp"``, ``"rawKeyDown"``, or ``"char"``.
            key: DOM ``KeyboardEvent.key`` value (e.g. ``"Enter"``).
            code: Physical key code (e.g. ``"KeyA"``).
            text: Character to insert (e.g. ``"a"``).
            modifiers: Bit field for modifier keys.
        """
        ...

class _AcquireContext:
    async def __aenter__(self) -> PooledTab: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...

class _PoolContext:
    async def __aenter__(self) -> BrowserPool: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...

class _PoolParamsContext:
    async def __aenter__(self) -> BrowserPool: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...

class BrowserPool:
    """Rust-side pool of reusable browser tabs (internal).

    Use the Python wrapper :class:`~voidcrawl.BrowserPool` instead.
    """

    @classmethod
    def from_env(cls) -> _PoolContext: ...
    @classmethod
    def _from_params(
        cls,
        browsers: int,
        tabs_per_browser: int,
        tab_max_uses: int,
        tab_max_idle_secs: int,
        headless: bool,
        no_sandbox: bool,
        stealth: bool,
        ws_urls: list[str],
        proxy: str | None,
        chrome_executable: str | None,
        extra_args: list[str],
    ) -> _PoolParamsContext: ...
    async def warmup(self) -> None: ...
    def acquire(self) -> _AcquireContext: ...
    async def release(self, tab: PooledTab) -> None: ...
    async def __aenter__(self) -> BrowserPool: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...

class Page:
    """A single browser tab created via :meth:`BrowserSession.new_page`."""

    async def goto(self, url: str, timeout: float = 30.0) -> PageResponse:
        """Navigate to *url* and wait for network idle in one shot."""
        ...
    async def navigate(self, url: str) -> None:
        """Navigate to *url* without waiting for any load event."""
        ...
    async def wait_for_navigation(self) -> None:
        """Block until the current navigation completes."""
        ...
    async def content(self) -> str:
        """Return the full page HTML."""
        ...
    async def title(self) -> str | None:
        """Return the document title, or ``None``."""
        ...
    async def url(self) -> str | None:
        """Return the current page URL, or ``None``."""
        ...
    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* and return the result."""
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes."""
        ...
    async def pdf_bytes(self) -> bytes:
        """Render the page as a PDF and return the raw bytes."""
        ...
    async def query_selector(self, selector: str) -> str | None:
        """Return inner HTML of the first matching element."""
        ...
    async def query_selector_all(self, selector: str) -> list[str]:
        """Return inner HTML of every matching element."""
        ...
    async def click_element(self, selector: str) -> None:
        """Click the first element matching *selector*."""
        ...
    async def type_into(self, selector: str, text: str) -> None:
        """Focus and type *text* into the first matching element."""
        ...
    async def set_headers(self, headers: dict[str, str]) -> None:
        """Set extra HTTP headers for subsequent requests."""
        ...
    async def get_cookies(self) -> list[dict[str, Any]]:
        """Return all cookies matching the current page URL."""
        ...
    async def set_cookie(
        self,
        name: str,
        value: str,
        *,
        domain: str | None = None,
        path: str | None = None,
        secure: bool | None = None,
        http_only: bool | None = None,
    ) -> None:
        """Set a cookie on the current page."""
        ...
    async def delete_cookie(
        self,
        name: str,
        *,
        domain: str | None = None,
        path: str | None = None,
    ) -> None:
        """Delete a cookie by name, optionally scoped to a domain and path."""
        ...
    async def wait_for_stable_dom(
        self, timeout: float = 10.0, min_length: int = 5000, stable_checks: int = 5
    ) -> bool:
        """Wait until the DOM stabilises (stops changing)."""
        ...
    async def wait_for_network_idle(self, timeout: float = 30.0) -> str | None:
        """Wait for network activity to settle."""
        ...
    async def dispatch_mouse_event(
        self,
        event_type: str,
        x: float,
        y: float,
        button: str = "left",
        click_count: int = 1,
        delta_x: float | None = None,
        delta_y: float | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchMouseEvent``."""
        ...
    async def dispatch_key_event(
        self,
        event_type: str,
        key: str | None = None,
        code: str | None = None,
        text: str | None = None,
        modifiers: int | None = None,
    ) -> None:
        """Send a low-level CDP ``Input.dispatchKeyEvent``."""
        ...
    async def close(self) -> None:
        """Close this tab and release its resources."""
        ...

class BrowserSession:
    """Rust-side browser session (internal).

    Use the Python wrapper :class:`~voidcrawl.BrowserSession` instead.
    """

    def __init__(
        self,
        *,
        headless: bool = True,
        ws_url: str | None = None,
        stealth: bool = True,
        no_sandbox: bool = False,
        proxy: str | None = None,
        chrome_executable: str | None = None,
        extra_args: list[str] | None = None,
    ) -> None: ...
    async def launch(self) -> None: ...
    async def new_page(self, url: str) -> Page: ...
    async def version(self) -> str: ...
    async def close(self) -> None: ...
    async def __aenter__(self) -> BrowserSession: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...
