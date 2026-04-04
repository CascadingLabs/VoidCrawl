"""Type stubs for the void_crawl._ext native extension module.

Internal — import from ``void_crawl`` instead.
"""

from __future__ import annotations

class PooledTab:
    """A tab checked out from a :class:`~void_crawl.BrowserPool`.

    Exposes the same page-interaction methods as :class:`Page` but must
    not be closed manually — return it to the pool via the async context
    manager or :meth:`~void_crawl.BrowserPool.release`.

    Attributes:
        use_count: How many times this tab has been acquired (0 on first use).
    """

    use_count: int

    async def goto(self, url: str, timeout: float = 30.0) -> str | None:
        """Navigate to *url* and wait for network idle in one shot.

        Args:
            url: The URL to load.
            timeout: Maximum seconds to wait for network idle.

        Returns:
            ``"networkIdle"`` or ``"networkAlmostIdle"`` on success,
            ``None`` on timeout.
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
        """Return the full page HTML (``document.documentElement.outerHTML``).

        Returns:
            The complete outer HTML of the document element.
        """
        ...
    async def title(self) -> str | None:
        """Return the document title, or ``None``.

        Returns:
            The ``document.title`` string, or ``None`` if unavailable.
        """
        ...
    async def url(self) -> str | None:
        """Return the current page URL, or ``None``.

        Returns:
            The page URL as a string, or ``None`` if unavailable.
        """
        ...
    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* and return the result.

        The return value is deserialised to a native Python type
        (``dict``, ``list``, ``str``, ``int``, ``float``, ``bool``, or ``None``).

        Args:
            expression: JavaScript expression or IIFE string.

        Returns:
            The deserialised result of the expression.
        """
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes.

        Returns:
            Raw PNG image data.
        """
        ...
    async def query_selector(self, selector: str) -> str | None:
        """Return the outer HTML of the first element matching *selector*, or ``None``.

        Args:
            selector: CSS selector string.

        Returns:
            Outer HTML string of the matched element, or ``None`` if no match.
        """
        ...
    async def query_selector_all(self, selector: str) -> list[str]:
        """Return the outer HTML of every element matching *selector*.

        Args:
            selector: CSS selector string.

        Returns:
            List of outer HTML strings, one per matched element.
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

    Use the Python wrapper :class:`~void_crawl.BrowserPool` instead.
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
    """A single browser tab created via :meth:`BrowserSession.new_page`.

    Provides navigation, content extraction, JavaScript evaluation,
    media capture, DOM queries, user-interaction helpers, and low-level
    CDP input dispatch.
    """

    async def goto(self, url: str, timeout: float = 30.0) -> str | None:
        """Navigate to *url* and wait for network idle in one shot.

        Args:
            url: The URL to load.
            timeout: Maximum seconds to wait for network idle.

        Returns:
            ``"networkIdle"`` or ``"networkAlmostIdle"`` on success,
            ``None`` on timeout.
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
        """Return the full page HTML (``document.documentElement.outerHTML``).

        Returns:
            The complete outer HTML of the document element.
        """
        ...
    async def title(self) -> str | None:
        """Return the document title, or ``None``.

        Returns:
            The ``document.title`` string, or ``None`` if unavailable.
        """
        ...
    async def url(self) -> str | None:
        """Return the current page URL, or ``None``.

        Returns:
            The page URL as a string, or ``None`` if unavailable.
        """
        ...
    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* and return the result.

        The return value is deserialised to a native Python type
        (``dict``, ``list``, ``str``, ``int``, ``float``, ``bool``, or ``None``).

        Args:
            expression: JavaScript expression or IIFE string.

        Returns:
            The deserialised result of the expression.
        """
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes.

        Returns:
            Raw PNG image data.
        """
        ...
    async def pdf_bytes(self) -> bytes:
        """Render the page as a PDF and return the raw bytes.

        Only works in headless mode.

        Returns:
            Raw PDF file data.
        """
        ...
    async def query_selector(self, selector: str) -> str | None:
        """Return the outer HTML of the first element matching *selector*, or ``None``.

        Args:
            selector: CSS selector string.

        Returns:
            Outer HTML string of the matched element, or ``None`` if no match.
        """
        ...
    async def query_selector_all(self, selector: str) -> list[str]:
        """Return the outer HTML of every element matching *selector*.

        Args:
            selector: CSS selector string.

        Returns:
            List of outer HTML strings, one per matched element.
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
        """Set extra HTTP headers for all subsequent requests from this page.

        Args:
            headers: Header name-value pairs.
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
    async def close(self) -> None:
        """Close this tab and release its resources."""
        ...

class BrowserSession:
    """Rust-side browser session (internal).

    Use the Python wrapper :class:`~void_crawl.BrowserSession` instead.
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
