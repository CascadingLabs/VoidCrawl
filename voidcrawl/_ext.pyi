"""Type stubs for the voidcrawl._ext native extension module.

Internal — import from ``voidcrawl`` instead.
"""

from __future__ import annotations

from typing import Any

class AntibotVerdict:
    """Signature-based anti-bot / CDN vendor fingerprint of a response.

    Attributes:
        vendors: Canonical vendor tags detected (e.g. ``"cloudflare"``,
            ``"datadome"``), sorted.
        challenged: ``True`` when an active wall/challenge fired (rotate),
            vs. mere CDN presence (no action needed).
        challenge_vendor: Vendor whose challenge fired, when ``challenged``.
        corpus_version: Signature corpus the verdict was produced against —
            record alongside captures for replay-grade provenance.
        evidence: Which tier matched — ``"none"`` / ``"headers"`` / ``"body"``.
    """

    vendors: list[str]
    challenged: bool
    challenge_vendor: str | None
    corpus_version: str
    evidence: str

class PageResponse:
    """Result of :meth:`Page.goto` / :meth:`PooledTab.goto`.

    Attributes:
        html: Full outer HTML after network idle.
        url: Final URL after any redirects.
        status_code: HTTP status of the last response, or ``None``
            when served from cache / service worker.
        redirected: ``True`` when at least one HTTP redirect occurred.
        headers: Final Document response headers (lowercased names; last
            value wins on duplicates). Empty when no network response was
            captured.
        antibot: Anti-bot / CDN vendor fingerprint, or ``None`` when no
            network response was captured.
    """

    html: str
    url: str
    status_code: int | None
    redirected: bool
    headers: dict[str, str]
    antibot: AntibotVerdict | None

class DownloadOutcome:
    """Result of :meth:`Page.download` / :meth:`PooledTab.download`.

    Attributes:
        path: Absolute path to the downloaded file.
        bytes: Size of the downloaded file in bytes.
        content_type: The server's ``Content-Type`` (parameters stripped), or
            ``None``. Pass to :func:`scan_file` as ``claimed_mime``.
    """

    path: str
    bytes: int
    content_type: str | None

class DownloadCapture:
    """Opaque handle for an armed action-triggered download.

    Created by :meth:`Page.arm_download` / :meth:`PooledTab.arm_download`; pass
    to the matching ``wait_download`` after performing the triggering action.
    """

class ScanReport:
    """Result of :func:`scan_file` / :func:`scan_bytes`.

    Attributes:
        verdict: ``"clean"`` or ``"flagged"``.
        is_clean: ``True`` iff ``verdict == "clean"``.
        reason: Why it was flagged (``None`` when clean).
        detected_mime: MIME inferred from the file's magic bytes.
        size: Size of the scanned buffer in bytes.
    """

    verdict: str
    is_clean: bool
    reason: str | None
    detected_mime: str | None
    size: int

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
    async def eval_js(self, expression: str) -> object:
        """Alias for :meth:`evaluate_js` — short form used by MCP tooling."""
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes."""
        ...
    async def download(
        self,
        url: str,
        dir: str,  # noqa: A002 — mirrors the native binding
        timeout: float = 120.0,
        max_bytes: int | None = None,
    ) -> DownloadOutcome:
        """Download *url* into directory *dir*; see :meth:`Page.download`."""
        ...
    async def arm_download(
        self,
        dir: str,  # noqa: A002 — mirrors the native binding
        max_bytes: int | None = None,
    ) -> DownloadCapture:
        """Arm an action-triggered download capture; see :meth:`Page.arm_download`."""
        ...
    async def wait_download(
        self, capture: DownloadCapture, timeout: float = 120.0
    ) -> DownloadOutcome:
        """Await an armed capture; see :meth:`Page.wait_download`."""
        ...
    async def reset_download(self) -> None:
        """Reset this tab's CDP download behavior to Chrome's default."""
        ...
    async def get_full_ax_tree(self, depth: int | None = None) -> list[dict[str, Any]]:
        """Return the browser-computed accessibility (AX) tree.

        Wraps CDP ``Accessibility.getFullAXTree``. The result is a flat list of
        AX node dicts linked by ``childIds``/``parentId``; each node carries
        ``role``, computed ``name``, ``properties`` (state), and
        ``backendDOMNodeId``. Call after the page has rendered.

        Args:
            depth: Maximum descendant depth to traverse. ``None`` returns the
                whole tree.
        """
        ...
    async def ax_tree_outline(self, depth: int | None = None) -> str:
        """Return the AX tree as a compact, indented ``role "name"`` outline.

        Readable counterpart to :meth:`get_full_ax_tree`: text-noise and hidden
        nodes are pruned. Same output the MCP ``session_ax_tree`` tool renders.
        """
        ...
    async def query_ax_tree(
        self, role: str | None = None, name: str | None = None
    ) -> list[dict[str, Any]]:
        """Query the AX tree (``Accessibility.queryAXTree``) for matching nodes.

        The semantic analogue of ``query_selector_all``: addresses by computed
        ``role`` / accessible ``name`` rather than markup. Name matching is
        exact. Passing neither returns every node under the document root.
        """
        ...
    async def click_by_role(self, role: str, name: str, nth: int = 0) -> None:
        """Click the *nth* element matching accessibility ``role`` + ``name``.

        Markup-independent analogue of ``click_element``: resolves via the AX
        tree, bridges to the DOM, scrolls into view, and clicks. Raises if no
        such node exists.

        Args:
            role: Computed accessibility role, e.g. ``"button"``, ``"link"``.
            name: Computed accessible name (exact match).
            nth: 0-based index when several nodes match.
        """
        ...
    async def set_geolocation(
        self, latitude: float, longitude: float, accuracy: float | None = None
    ) -> None:
        """Override geolocation and grant the geolocation permission.

        ``navigator.geolocation`` reads require a secure context (https /
        localhost), not ``data:`` URLs. ``accuracy`` defaults to 50 metres.
        """
        ...
    async def set_locale(self, locale: str) -> None:
        """Override the locale (Intl + ``Accept-Language``), e.g. ``"fr-FR"``."""
        ...
    async def set_timezone(self, timezone_id: str) -> None:
        """Override the timezone by IANA id, e.g. ``"America/New_York"``."""
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
    async def wait_for_network_idle(self, timeout: float = 30.0) -> str | None:
        """Wait for network activity to settle.

        Args:
            timeout: Maximum seconds to wait.

        Returns:
            ``"networkIdle"`` or ``"networkAlmostIdle"`` on success,
            ``None`` on timeout.
        """
        ...
    async def wait_for_selector(self, selector: str, timeout: float = 30.0) -> None:
        """Wait until a CSS selector matches. Event-driven — no polling.

        Raises :class:`VoidCrawlError` if *timeout* seconds elapse
        without a match.
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
    async def eval_js(self, expression: str) -> object:
        """Alias for :meth:`evaluate_js` — short form used by MCP tooling."""
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes."""
        ...
    async def screenshot(
        self,
        path: str | None = None,
        bbox: tuple[int, int, int, int] | None = None,
    ) -> bytes | str:
        """Capture a PNG screenshot with optional disk output and/or crop.

        Args:
            path: If set, writes PNG to this path and returns the path.
                If omitted, returns raw bytes.
            bbox: Optional ``(x, y, width, height)`` in CSS pixels.
        """
        ...
    async def detect_captcha(self) -> str | None:
        """Probe DOM for captcha / bot-wall markers.

        Returns one of ``"recaptcha"``, ``"hcaptcha"``, ``"turnstile"``,
        ``"cloudflare_challenge"``, ``"datadome"`` — or ``None``.
        """
        ...
    async def pdf_bytes(self) -> bytes:
        """Render the page as a PDF and return the raw bytes."""
        ...
    async def download(
        self,
        url: str,
        dir: str,  # noqa: A002 — mirrors the native binding
        timeout: float = 120.0,
        max_bytes: int | None = None,
    ) -> DownloadOutcome:
        """Download *url* into directory *dir* through this page's browser
        context (cookies / fingerprint preserved).

        The stream aborts past *max_bytes*. Treat *dir* as quarantine and pass
        the result to :func:`scan_file` before trusting the file. The CDP
        download behavior is reset before this returns.

        Args:
            url: Absolute URL of the file to download.
            dir: Directory the file is saved into.
            timeout: Download timeout in seconds.
            max_bytes: Abort past this many bytes (default 100 MiB).
        """
        ...
    async def arm_download(
        self,
        dir: str,  # noqa: A002 — mirrors the native binding
        max_bytes: int | None = None,
    ) -> DownloadCapture:
        """Arm an action-triggered download capture into *dir*.

        Perform the triggering action next (e.g. :meth:`click_by_role`), then
        pass the returned capture to :meth:`wait_download`. Use for downloads
        started by a page action — a "Download" button, a generated/cross-origin
        URL (Google Drive) — rather than :meth:`download`, which needs a URL.
        :func:`voidcrawl.capture_download` brackets these as a context manager.
        """
        ...
    async def wait_download(
        self, capture: DownloadCapture, timeout: float = 120.0
    ) -> DownloadOutcome:
        """Wait for the armed *capture* to land a new download. Resets the
        page's download behavior. The capture is consumed (single wait)."""
        ...
    async def reset_download(self) -> None:
        """Reset this page's CDP download behavior to Chrome's default. Call to
        release an armed-but-unused capture (e.g. on an error path)."""
        ...
    async def get_full_ax_tree(self, depth: int | None = None) -> list[dict[str, Any]]:
        """Return the browser-computed accessibility (AX) tree.

        Wraps CDP ``Accessibility.getFullAXTree``. The result is a flat list of
        AX node dicts linked by ``childIds``/``parentId``; each node carries
        ``role``, computed ``name``, ``properties`` (state), and
        ``backendDOMNodeId``. Call after the page has rendered.

        Args:
            depth: Maximum descendant depth to traverse. ``None`` returns the
                whole tree.
        """
        ...
    async def ax_tree_outline(self, depth: int | None = None) -> str:
        """Return the AX tree as a compact, indented ``role "name"`` outline.

        Readable counterpart to :meth:`get_full_ax_tree`: text-noise and hidden
        nodes are pruned. Same output the MCP ``session_ax_tree`` tool renders.
        """
        ...
    async def query_ax_tree(
        self, role: str | None = None, name: str | None = None
    ) -> list[dict[str, Any]]:
        """Query the AX tree (``Accessibility.queryAXTree``) for matching nodes.

        The semantic analogue of ``query_selector_all``: addresses by computed
        ``role`` / accessible ``name`` rather than markup. Name matching is
        exact. Passing neither returns every node under the document root.
        """
        ...
    async def click_by_role(self, role: str, name: str, nth: int = 0) -> None:
        """Click the *nth* element matching accessibility ``role`` + ``name``.

        Markup-independent analogue of ``click_element``: resolves via the AX
        tree, bridges to the DOM, scrolls into view, and clicks. Raises if no
        such node exists.

        Args:
            role: Computed accessibility role, e.g. ``"button"``, ``"link"``.
            name: Computed accessible name (exact match).
            nth: 0-based index when several nodes match.
        """
        ...
    async def set_geolocation(
        self, latitude: float, longitude: float, accuracy: float | None = None
    ) -> None:
        """Override geolocation and grant the geolocation permission.

        ``navigator.geolocation`` reads require a secure context (https /
        localhost), not ``data:`` URLs. ``accuracy`` defaults to 50 metres.
        """
        ...
    async def set_locale(self, locale: str) -> None:
        """Override the locale (Intl + ``Accept-Language``), e.g. ``"fr-FR"``."""
        ...
    async def set_timezone(self, timezone_id: str) -> None:
        """Override the timezone by IANA id, e.g. ``"America/New_York"``."""
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
    async def wait_for_network_idle(self, timeout: float = 30.0) -> str | None:
        """Wait for network activity to settle."""
        ...
    async def wait_for_selector(self, selector: str, timeout: float = 30.0) -> None:
        """Wait until a CSS selector matches. Event-driven — no polling."""
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

# ── Profiles ────────────────────────────────────────────────────────────

class ProfileHandle:
    """Live lease on a Chrome profile.

    Use as an async context manager, or call :meth:`release` explicitly.
    Obtain one via :func:`voidcrawl.acquire_profile` or
    :func:`voidcrawl.with_profile`.
    """

    name: str
    async def path(self) -> str: ...
    async def new_page(self, url: str) -> Page: ...
    async def release(self) -> None: ...
    async def __aenter__(self) -> ProfileHandle: ...
    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> None: ...

def py_list_profiles() -> list[tuple[str, str]]: ...
async def py_acquire_profile(
    name: str,
    lease_timeout: float = 300.0,
    headless: bool = True,
) -> ProfileHandle: ...

# ── Scanner ─────────────────────────────────────────────────────────────

def scan_file(
    path: str,
    max_bytes: int | None = None,
    claimed_mime: str | None = None,
) -> ScanReport:
    """Scan a file on disk with the content-safety gate (size cap + magic-byte
    type check + yara-x signatures). Returns a :class:`ScanReport`."""
    ...

def scan_bytes(
    data: bytes,
    max_bytes: int | None = None,
    claimed_mime: str | None = None,
) -> ScanReport:
    """Scan an in-memory buffer with the content-safety gate. See
    :func:`scan_file`."""
    ...

# ── Exceptions ──────────────────────────────────────────────────────────
# ruff: noqa: N818  — these are the public exception names, preserved for API compat

class VoidCrawlError(Exception):
    """Base class for all voidcrawl errors raised from the native extension."""

class ProfileBusy(VoidCrawlError):
    """Another voidcrawl process holds the profile lock (non-blocking acquire)."""

class ProfileLeaseExpired(VoidCrawlError):
    """Timed out waiting for the profile lock."""

class ProfileNotFound(VoidCrawlError):
    """No matching profile directory in the platform default dirs."""

class CaptchaDetected(VoidCrawlError):
    """DOM markers indicate a captcha / bot-wall challenge on the page."""

class AntibotChallenge(VoidCrawlError):
    """An anti-bot vendor is actively challenging the response.

    Signature-based (header/status/body), distinct from the DOM-based
    :class:`CaptchaDetected`. Not raised on the ``fetch`` / ``fetch_many``
    path — those surface the verdict as the non-fatal
    :attr:`PageResponse.antibot` annotation instead; this is reserved for
    explicit detect/routing callers that opt into failing on a wall.
    """
