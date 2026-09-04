"""Type stubs for the voidcrawl._ext native extension module.

Internal — import from ``voidcrawl`` instead.
"""

from __future__ import annotations

from typing import Any, Literal

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
        endpoints: Data-plane network endpoints (XHR + Fetch request URLs) —
            a sorted, deduplicated set of ``scheme://host/path`` with query,
            fragment, and userinfo stripped and secret-like path segments
            redacted at the source. ``None`` unless ``capture_endpoints=True``
            was passed to ``goto``; ``[]`` when requested but none were seen.
        endpoints_truncated: ``True`` when the endpoint set hit its cap and
            further endpoints were dropped.
        endpoint_sanitizer_version: Which redaction-rule version produced
            ``endpoints`` (record it alongside the set for replay-grade
            provenance). ``None`` iff ``endpoints`` is ``None``.
    """

    html: str
    url: str
    status_code: int | None
    redirected: bool
    headers: dict[str, str]
    antibot: AntibotVerdict | None
    endpoints: list[str] | None
    endpoints_truncated: bool
    endpoint_sanitizer_version: str | None

class CapturedResponse:
    """A passively observed response with an opt-in bounded body."""

    url: str
    status: int
    headers: dict[str, str]
    request_headers: dict[str, str]
    mime_type: str
    resource_type: str
    from_cache: bool
    from_service_worker: bool
    body_state: str
    body_error: str | None
    truncated: bool
    async def bytes(self) -> bytes: ...
    async def text(self) -> str: ...
    async def json(self) -> Any: ...

class ResponseExpectation:
    """Async context returned by ``Page`` or ``PooledTab.expect_response(s)``."""

    async def __aenter__(self) -> ResponseExpectation: ...
    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> bool: ...
    @property
    def value(self) -> Any: ...

class RenderedDomSnapshot:
    state: str
    unavailable_reason: str | None
    epoch: int | None
    frame_scope: str
    url: str | None
    generated_at_unix_ms: int | None
    retained_bytes: int
    complete_bytes: int | None
    def bytes(self) -> bytes: ...

class AccessibilitySnapshot:
    state: str
    unavailable_reason: str | None
    epoch: int | None
    frame_scope: str
    frame_url: str | None
    url: str | None
    generated_at_unix_ms: int | None
    requested_depth: int | None
    nodes_observed: int
    nodes_retained: int
    retained_bytes: int
    complete_bytes: int | None
    def bytes(self) -> bytes: ...

class LayoutSnapshot:
    epoch: int | None
    url: str | None
    generated_at_unix_ms: int | None
    layout_viewport: tuple[int, int, int, int]
    visual_viewport: tuple[
        float, float, float, float, float, float, float, float | None
    ]
    content_size: tuple[float, float, float, float]
    device_scale_factor: float | None

class VisualSnapshot:
    epoch: int | None
    url: str | None
    generated_at_unix_ms: int | None
    format: str
    region: str
    bbox: tuple[int, int, int, int] | None
    target_kind: str | None
    image_size: tuple[int, int]
    capture_viewport: tuple[int, int]
    device_scale_factor: float
    retained_bytes: int
    complete: bool
    def bytes(self) -> bytes: ...

class NavigationCaptureReport:
    """Terminal source/resource graph report with a redacted representation."""

    termination: str
    started_at_unix_ms: int | None
    elapsed_micros: int
    events_admitted: int
    resources_dropped: int
    additional_loss_unknown: bool
    cleanup_complete: bool
    network_extra_info: str
    requested_url: str | None
    final_url: str | None
    redirect_count: int
    resource_count: int
    source_status: int | None
    source_body_state: str | None
    source_retained_bytes: int | None
    source_complete_bytes: int | None
    def source_body(self) -> bytes | None: ...
    def source_header_names(self) -> list[str]: ...
    def resources(self, *, include_urls: bool = False) -> list[dict[str, Any]]: ...

class NavigationCapture:
    """Armed main-document source and resource-graph capture."""

    async def finish(self) -> NavigationCaptureReport: ...
    async def cancel(self) -> NavigationCaptureReport: ...
    async def wait(self) -> NavigationCaptureReport: ...

class ObservationScope:
    """Armed, bounded CDP lifecycle observation."""

    async def finish(self) -> dict[str, Any]: ...
    async def cancel(self) -> dict[str, Any]: ...
    async def interrupt(self) -> dict[str, Any]: ...
    async def wait(self) -> dict[str, Any]: ...

class TabInstrumentationState:
    """Per-tab CDP instrumentation state for routing sensitive work.

    Attributes:
        low_cdp: ``True`` while the tab has not enabled higher-signal CDP domains.
        network_enabled: ``True`` after ``Network.enable`` has been sent.
        runtime_enabled: ``True`` after ``Runtime.enable`` has been sent for
            frame-scoped JavaScript.
        utility_world_enabled: Reserved for future isolated-world tracking.
        pre_navigation_stealth: ``True`` if VoidCrawl applied UA/viewport
            pre-navigation stealth to this tab.
    """

    low_cdp: bool
    network_enabled: bool
    runtime_enabled: bool
    utility_world_enabled: bool
    pre_navigation_stealth: bool

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

class PoolReleaseReport:
    state_binding: str
    strategy: str
    cleanup_complete: bool
    tab_reused: bool
    document_cleared: bool
    download_behavior_reset: bool
    shared_state_retained: bool

class PooledTab:
    """A tab checked out from a :class:`~voidcrawl.BrowserPool`.

    Exposes the same page-interaction methods as :class:`Page` but must
    not be closed manually — return it to the pool via the async context
    manager or :meth:`~voidcrawl.BrowserPool.release`.

    Attributes:
        use_count: How many times this tab has been acquired (0 on first use).
    """

    use_count: int

    async def goto(
        self, url: str, timeout: float = 30.0, capture_endpoints: bool = False
    ) -> PageResponse:
        """Navigate to *url* and wait for network idle in one shot.

        Args:
            url: The URL to load.
            timeout: Maximum seconds to wait for network idle.
            capture_endpoints: When ``True``, record the data-plane network
                endpoints (XHR + Fetch request URLs) seen during the load
                and surface them as :attr:`PageResponse.endpoints` —
                sanitized (query/fragment/userinfo stripped, secret-like
                path segments redacted), deduplicated, sorted, and capped.

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
    def expect_response(
        self,
        pattern: str,
        timeout: float = 30.0,
        max_response_bytes: int = 2097152,
        max_total_bytes: int = 8388608,
    ) -> ResponseExpectation:
        """Arm one bounded passive response expectation before an action."""
        ...
    def expect_responses(
        self,
        patterns: dict[str, str],
        timeout: float = 30.0,
        max_response_bytes: int = 2097152,
        max_total_bytes: int = 8388608,
    ) -> ResponseExpectation:
        """Arm named bounded passive response expectations before an action."""
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
    async def state_binding(self) -> str: ...
    async def instrumentation_state(self) -> TabInstrumentationState:
        """Return this tab's CDP instrumentation state."""
        ...
    async def environment_snapshot(self) -> dict[str, Any]:
        """Return effective browser environment and capture capabilities."""
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
    async def evaluate_js_in_frame(
        self, frame_url_pattern: str, expression: str
    ) -> object:
        """Evaluate *expression* inside a (possibly cross-origin) iframe.

        The frame is selected by a substring of its URL. The expression runs in
        that frame's own execution context (``document`` is the frame's
        document) — the way to reach an iframe whose ``contentDocument`` is null
        from the parent under the same-origin policy.

        Args:
            frame_url_pattern: Substring of the target frame's URL,
                e.g. ``"recaptcha/api2/bframe"``.
            expression: JavaScript expression or IIFE string.

        Raises:
            RuntimeError: if no frame matches, or the matched frame has no
                scriptable execution context.
        """
        ...
    async def eval_js_in_frame(self, frame_url_pattern: str, expression: str) -> object:
        """Alias for :meth:`evaluate_js_in_frame`."""
        ...
    async def frame_urls(self) -> list[str]:
        """List the URLs of every frame on the page, in no particular order.

        Handy for discovering the right ``frame_url_pattern`` to pass to
        :meth:`evaluate_js_in_frame`.
        """
        ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes."""
        ...
    async def screenshot(
        self,
        path: str | None = None,
        bbox: tuple[int, int, int, int] | None = None,
        selector_type: str | None = None,
        selector_value: str | None = None,
        selector_regex: str | None = None,
        selector_name: str | None = None,
        selector_nth: int | None = None,
        selector_x: float | None = None,
        selector_y: float | None = None,
        viewport_preset: str | None = None,
        viewport_width: int | None = None,
        viewport_height: int | None = None,
        viewport_device_scale_factor: float | None = None,
        viewport_mobile: bool | None = None,
        scroll_viewports: float | None = None,
        scroll_pixels: int | None = None,
        full_page: bool | None = None,
    ) -> bytes | str:
        """Capture a PNG screenshot; see :meth:`Page.screenshot` for the
        full argument reference (including the ``selector_*`` kwargs — a
        one-shot selector-backed crop is safe on a pooled tab). No
        persistent ``set_viewport`` exists on a pooled tab — the pool
        doesn't reset viewport on release, so a persistent override would
        leak to the next unrelated caller that acquires this tab. Use the
        one-shot ``viewport_*`` kwargs here instead."""
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
    async def click_by_role(
        self, role: str, name: str, nth: int = 0, humanize: bool = False
    ) -> None:
        """Click the *nth* element matching accessibility ``role`` + ``name``.

        Markup-independent analogue of ``click_element``: resolves via the AX
        tree, bridges to the DOM, scrolls into view, and clicks. Raises if no
        such node exists.

        Args:
            role: Computed accessibility role, e.g. ``"button"``, ``"link"``.
            name: Computed accessible name (exact match).
            nth: 0-based index when several nodes match.
            humanize: Click at the element's box-model centre with a humanized
                compositor pointer path (curved, min-jerk, tremor) instead of a
                DOM ``.click()``. Off by default.
        """
        ...
    async def move_mouse(self, x: float, y: float, humanize: bool = False) -> None:
        """Move the virtual cursor to ``(x, y)`` via CDP ``Input.dispatchMouseEvent``.

        With ``humanize=True`` the cursor travels a realistic curved, minimum-jerk,
        lightly-tremored path (multiple ``MouseMoved`` events) from its last
        position; otherwise it jumps in one event. No page-world JS is injected."""
        ...
    async def click_xy(self, x: float, y: float, humanize: bool = False) -> None:
        """Click at ``(x, y)`` with a trusted compositor event (press → release).

        With ``humanize=True`` the cursor first travels a human-like path there
        (see :meth:`move_mouse`). The programmatic analogue of the
        ``click_visual_coords`` MCP tool."""
        ...
    async def click_ax_in_frame(
        self,
        frame_url_pattern: str,
        role: str,
        name: str,
        nth: int = 0,
        humanize: bool = False,
    ) -> None:
        """Click an element by AX ``role`` + ``name`` inside a specific frame.

        The cross-frame, shadow-piercing analogue of :meth:`click_by_role`:
        roots the AX tree at the frame matched by ``frame_url_pattern`` and
        descends into closed shadow roots, then clicks the match at its
        box-model centre with a real **compositor** mouse event (a trusted
        click, unlike a DOM ``.click()``). Reaches widgets the page's own JS
        cannot — e.g. Cloudflare Turnstile's "Verify you are human" checkbox in
        a closed shadow root inside a cross-origin ``challenges.cloudflare.com``
        iframe. Empty ``name`` matches any node of that ``role``.

        Cross-origin google.com / cloudflare frames must be in-process: launch
        the session with ``extra_args=["disable-site-isolation-trials"]``.
        """
        ...
    async def ax_box_in_frame(
        self, frame_url_pattern: str, role: str, name: str, nth: int = 0
    ) -> list[float]:
        """Locate an AX ``role`` + ``name`` inside a frame; return its on-page
        rectangle ``[x, y, width, height]`` in CSS pixels.

        Same cross-frame, closed-shadow-piercing resolution as
        :meth:`click_ax_in_frame`, but returns the geometry instead of clicking
        — so you can drive a **humanized** click yourself (curved approach via
        :meth:`dispatch_mouse_event`, press at a jittered point in the box).
        Empty ``name`` matches any node of that ``role``."""
        ...
    async def ax_outline_in_frame(
        self, frame_url_pattern: str, depth: int | None = None
    ) -> str:
        """Compact accessibility outline of a specific (possibly cross-origin)
        frame — pierces closed shadow roots. Discover the role / accessible name
        to pass to :meth:`click_ax_in_frame`."""
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
        user_data_dir: str | None,
        cdp_mode: Literal["normal", "minimal"] | None = None,
    ) -> _PoolParamsContext: ...
    async def warmup(self) -> None: ...
    def acquire(self) -> _AcquireContext: ...
    async def release(self, tab: PooledTab) -> PoolReleaseReport | None: ...
    async def __aenter__(self) -> BrowserPool: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...

class Frame:
    """One captured frame of a :class:`Recording`."""

    index: int
    """Position in the sequence, 0-based."""
    offset_ms: float
    """Real elapsed milliseconds since the recording started. Frames are
    **not** evenly spaced — encode against this, not ``index / fps``."""
    data: bytes
    """Encoded image bytes, in the recording's ``format``."""

    def __len__(self) -> int: ...

class RecordedRegion:
    """One recorded region: the viewport, a ``bbox``, or one selector."""

    label: str
    """``"viewport"``, ``"bbox"``, or a name derived from the selector."""
    bbox: tuple[int, int, int, int] | None
    """``(x, y, width, height)`` in CSS pixels, resolved once at start."""
    frames: list[Frame]
    outputs: list[str]
    """Paths of encoded artifacts written for this region."""

class MaskReport:
    """What one mask of a :class:`Recording` covered.

    The library covers the rectangles it is given and reports the result. It
    does not decide what is sensitive, so a recording with masks is not
    thereby a safe-to-share one — ``unresolved_ticks`` and ``stale_frames``
    are here so you can make that call.
    """

    label: str
    bbox: tuple[int, int, int, int]
    """``(x, y, width, height)`` in CSS pixels, as first resolved."""
    tracked: bool
    """Whether the mask was re-resolved while recording."""
    unresolved_ticks: int
    """Ticks where re-resolution failed. The mask kept its last known
    rectangle for those, so something stayed covered."""
    stale_frames: int
    """Frames captured while the most recent re-resolution had failed."""

class Recording:
    """The result of :meth:`Page.record` / :meth:`RecordingHandle.stop`."""

    started_at_unix_ms: int | None
    document_epoch: int | None
    regions: list[RecordedRegion]
    """One per requested region; a single ``"viewport"`` region when neither
    ``bbox`` nor ``selectors`` was given."""
    masks: list[MaskReport]
    """One per requested mask. Empty means nothing was asked to be covered —
    not that there was nothing worth covering."""
    format: str
    """``"jpeg"`` or ``"png"``."""
    duration_ms: float
    frames_captured: int
    frames_dropped: int
    """Frames discarded by bounds or invalid frame data."""
    frames_dropped_by_rate: int
    frames_dropped_by_limit: int
    frame_decode_failures: int
    frame_ack_failures: int
    stream_disconnected: bool
    complete: bool
    frame_size_pixels: tuple[int, int] | None
    capture_viewport_css: tuple[float, float] | None
    device_pixel_ratio: float
    foregrounded: bool
    """Whether the recording pinned its tab to the foreground and held the
    browser's capture lock. ``False`` means it ran concurrently."""

    def effective_fps(self) -> float:
        """Frames per second actually achieved — at most the requested
        ``fps``, and usually below it on a mostly-static page."""
        ...

class RecordingHandle:
    """A recording in flight, from :meth:`Page.start_recording`."""

    async def stop(self) -> Recording:
        """Stop the recording and return the :class:`Recording`. Restores
        viewport and scroll position, releases the capture lock if one was
        taken, then crops and encodes. Raises on a second call."""
        ...

class Page:
    """A single browser tab created via :meth:`BrowserSession.new_page`."""

    async def target_id(self) -> str:
        """Return the CDP target id of this page (stable across same-tab
        navigations). Pass to :meth:`BrowserSession.attach_page` to re-adopt
        this exact tab from another connection."""
        ...
    async def goto(
        self,
        url: str,
        timeout: float = 30.0,
        capture_endpoints: bool = False,
        *,
        wait_until: str = "networkidle",
    ) -> PageResponse:
        """Navigate to *url* and wait for network idle in one shot.

        Args:
            url: The URL to load.
            timeout: Maximum seconds to wait for network idle.
            capture_endpoints: When ``True``, record the data-plane network
                endpoints (XHR + Fetch request URLs) seen during the load
                and surface them as :attr:`PageResponse.endpoints`.
        """
        ...
    async def navigate(self, url: str) -> None:
        """Navigate to *url* without waiting for any load event."""
        ...
    async def add_init_script(self, script: str) -> None:
        """Install JavaScript before each subsequent document executes."""
        ...
    async def arm_navigation_capture(
        self,
        *,
        max_events: int = 4096,
        max_resources: int = 512,
        max_source_bytes: int = 8388608,
        max_duration: float = 30.0,
    ) -> NavigationCapture:
        """Arm main-document source and resource-graph capture before navigation."""
        ...
    async def arm_observation(
        self,
        *,
        collect_network: bool = True,
        collect_console: bool = True,
        collect_exceptions: bool = True,
        max_events: int = 2048,
        max_diagnostic_bytes: int = 65536,
        max_duration: float = 30.0,
    ) -> ObservationScope:
        """Arm bounded lifecycle markers before navigation or an action."""
        ...
    def expect_response(
        self,
        pattern: str,
        timeout: float = 30.0,
        max_response_bytes: int = 2097152,
        max_total_bytes: int = 8388608,
    ) -> ResponseExpectation: ...
    def expect_responses(
        self,
        patterns: dict[str, str],
        timeout: float = 30.0,
        max_response_bytes: int = 2097152,
        max_total_bytes: int = 8388608,
    ) -> ResponseExpectation: ...
    async def wait_for_navigation(self) -> None:
        """Block until the current navigation completes."""
        ...
    async def content(self) -> str:
        """Return the full page HTML."""
        ...
    async def rendered_dom_snapshot(
        self, max_bytes: int = 8388608
    ) -> RenderedDomSnapshot: ...
    async def accessibility_snapshot(
        self,
        depth: int | None = None,
        max_nodes: int = 10000,
        max_bytes: int = 8388608,
    ) -> AccessibilitySnapshot: ...
    async def accessibility_snapshot_in_frame(
        self,
        frame_url_pattern: str,
        depth: int | None = None,
        max_nodes: int = 10000,
        max_bytes: int = 8388608,
    ) -> AccessibilitySnapshot: ...
    async def title(self) -> str | None:
        """Return the document title, or ``None``."""
        ...
    async def url(self) -> str | None:
        """Return the current page URL, or ``None``."""
        ...
    async def state_binding(self) -> str: ...
    async def instrumentation_state(self) -> TabInstrumentationState:
        """Return this tab's CDP instrumentation state."""
        ...
    async def environment_snapshot(self) -> dict[str, Any]:
        """Return effective browser environment and capture capabilities."""
        ...
    async def evaluate_js(self, expression: str) -> object:
        """Evaluate a JavaScript *expression* and return the result."""
        ...
    async def eval_js(self, expression: str) -> object:
        """Alias for :meth:`evaluate_js` — short form used by MCP tooling."""
        ...
    async def evaluate_js_in_frame(
        self, frame_url_pattern: str, expression: str
    ) -> object:
        """Evaluate *expression* inside a (possibly cross-origin) iframe.

        The frame is selected by a substring of its URL. The expression runs in
        that frame's own execution context (``document`` is the frame's
        document) — the way to reach an iframe whose ``contentDocument`` is null
        from the parent under the same-origin policy.

        Args:
            frame_url_pattern: Substring of the target frame's URL,
                e.g. ``"recaptcha/api2/bframe"``.
            expression: JavaScript expression or IIFE string.

        Raises:
            RuntimeError: if no frame matches, or the matched frame has no
                scriptable execution context.
        """
        ...
    async def eval_js_in_frame(self, frame_url_pattern: str, expression: str) -> object:
        """Alias for :meth:`evaluate_js_in_frame`."""
        ...
    async def frame_urls(self) -> list[str]:
        """List the URLs of every frame on the page, in no particular order.

        Handy for discovering the right ``frame_url_pattern`` to pass to
        :meth:`evaluate_js_in_frame`.
        """
        ...
    async def layout_snapshot(self) -> LayoutSnapshot: ...
    async def visual_snapshot(
        self,
        bbox: tuple[int, int, int, int] | None = None,
        selector_type: str | None = None,
        selector_value: str | None = None,
        selector_regex: str | None = None,
        selector_name: str | None = None,
        selector_nth: int | None = None,
        selector_x: float | None = None,
        selector_y: float | None = None,
        viewport_preset: str | None = None,
        viewport_width: int | None = None,
        viewport_height: int | None = None,
        viewport_device_scale_factor: float | None = None,
        viewport_mobile: bool | None = None,
        scroll_viewports: float | None = None,
        scroll_pixels: int | None = None,
        full_page: bool | None = None,
    ) -> VisualSnapshot: ...
    async def screenshot_png(self) -> bytes:
        """Capture a full-page screenshot as PNG bytes."""
        ...
    async def record(
        self,
        duration_secs: float | None = None,
        selectors: list[dict[str, object]] | None = None,
        bbox: tuple[int, int, int, int] | None = None,
        masks: list[dict[str, object]] | None = None,
        mask_pad: int | None = None,
        viewport_preset: str | None = None,
        viewport_width: int | None = None,
        viewport_height: int | None = None,
        viewport_device_scale_factor: float | None = None,
        viewport_mobile: bool | None = None,
        scroll_viewports: float | None = None,
        scroll_pixels: int | None = None,
        fps: int | None = None,
        max_frames: int | None = None,
        frame_format: str | None = None,
        quality: int | None = None,
        output_dir: str | None = None,
        write_frames: bool | None = None,
        foreground: bool | None = None,
        encode: list[str] | None = None,
    ) -> Recording:
        """Record this page for ``duration_secs`` seconds.

        The moving-picture counterpart to :meth:`screenshot`, with the same
        ``viewport_*`` / ``scroll_*`` / ``bbox`` kwargs. Two differences,
        both forced by CDP's screencast:

        * No ``full_page`` — a screencast only ever contains the viewport.
        * ``selectors`` is a **list**: each entry becomes its own cropped
          region in ``recording.regions``, all cut from one screencast, each
          resolved to a rectangle once at start and then held fixed.

        Frames arrive when Chrome paints rather than on a clock, so ``fps``
        is a ceiling, not a guarantee, and a static page yields very few
        frames. ``bbox`` here is viewport-relative (a screencast frame only
        contains the viewport), unlike :meth:`screenshot`'s page-relative
        one.

        ``masks`` blacks out rectangles in every frame before anything is
        cropped, written, or encoded. Each entry is a selector dict
        (``{"type": "css", "value": "#password"}``) or a mask dict
        (``{"bbox": (x, y, w, h)}`` / ``{"selector": {...}, "track": False,
        "label": "pw"}``). Orthogonal to ``bbox``/``selectors``: crop to the
        form and mask a field inside it. Unlike a crop region, a selector
        mask is re-resolved while recording so it keeps covering an element
        that moves, and one that resolves to nothing fails the call rather
        than leaving a hole. See ``recording.masks`` for what each one did.

        This is a geometric primitive, not a redaction policy: it covers
        exactly what you name and reports what it covered. It does not
        decide what is sensitive.

        ``encode`` (``"gif"`` / ``"mp4"`` / ``"webm"``) requires ``output_dir``
        and
        the matching cargo feature; without it this raises rather than
        silently producing nothing, and the frames remain available.

        Leave ``foreground`` unset to detect it: a tab sharing its window
        must be foregrounded (holding the browser's capture lock) to paint
        at all, while a tab alone in its window records at full rate
        concurrently.
        """
        ...
    async def start_recording(
        self,
        duration_secs: float | None = None,
        selectors: list[dict[str, object]] | None = None,
        bbox: tuple[int, int, int, int] | None = None,
        masks: list[dict[str, object]] | None = None,
        mask_pad: int | None = None,
        viewport_preset: str | None = None,
        viewport_width: int | None = None,
        viewport_height: int | None = None,
        viewport_device_scale_factor: float | None = None,
        viewport_mobile: bool | None = None,
        scroll_viewports: float | None = None,
        scroll_pixels: int | None = None,
        fps: int | None = None,
        max_frames: int | None = None,
        frame_format: str | None = None,
        quality: int | None = None,
        output_dir: str | None = None,
        write_frames: bool | None = None,
        foreground: bool | None = None,
        encode: list[str] | None = None,
    ) -> RecordingHandle:
        """Begin recording and return a handle to stop it.

        Use instead of :meth:`record` when you need to drive the page while
        it records. Same kwargs, except ``duration_secs`` becomes a hard
        upper bound rather than the exact length.
        """
        ...
    async def alone_in_window(self) -> bool:
        """Whether this tab is the only one in its browser window.

        Chrome composites only a window's frontmost tab, so a page sharing
        its window can't paint while a sibling is active. A page alone in
        its window keeps painting — which is what lets :meth:`record` run
        without holding the browser's capture lock.
        """
        ...
    async def screenshot(
        self,
        path: str | None = None,
        bbox: tuple[int, int, int, int] | None = None,
        selector_type: str | None = None,
        selector_value: str | None = None,
        selector_regex: str | None = None,
        selector_name: str | None = None,
        selector_nth: int | None = None,
        selector_x: float | None = None,
        selector_y: float | None = None,
        viewport_preset: str | None = None,
        viewport_width: int | None = None,
        viewport_height: int | None = None,
        viewport_device_scale_factor: float | None = None,
        viewport_mobile: bool | None = None,
        scroll_viewports: float | None = None,
        scroll_pixels: int | None = None,
        full_page: bool | None = None,
    ) -> bytes | str:
        """Capture a PNG screenshot with optional disk output, cropping, a
        one-shot device/viewport override, scrolling, and/or viewport-only
        capture. Prefer building a validated
        :class:`voidcrawl.viewport.Viewport` and unpacking it with
        ``**vp.as_kwargs(prefix="viewport_")`` over passing these directly.

        Args:
            path: If set, writes PNG to this path and returns the path.
                If omitted, returns raw bytes.
            bbox: Optional ``(x, y, width, height)`` in CSS pixels. With
                ``scroll_viewports``/``scroll_pixels`` set, coordinates are
                relative to wherever that scroll lands. Mutually exclusive
                with ``selector_type``.
            selector_type: Crop to a Yosoi selector's resolved rectangle
                instead of an explicit ``bbox`` — one of ``"css"``,
                ``"xpath"``, ``"regex"``, ``"jsonld"``, ``"attr"``,
                ``"global_id"``, ``"role"``, ``"visual"``. Mutually
                exclusive with ``bbox``. A selector that matches nothing,
                is ambiguous, or is inherently non-visual (``jsonld``/
                ``regex``) raises rather than silently cropping an
                arbitrary target.
            selector_value: CSS selector / XPath expression, depending on
                ``selector_type`` (unused for ``role``/``visual``/
                ``jsonld``/``regex``).
            selector_regex: Regex pattern (``selector_type="regex"`` only —
                currently always resolves to "empty"; not cropped).
            selector_name: Accessible name (``role``), attribute name
                (``attr`` — metadata only, not part of the DOM query), or
                id-prefix filter (``global_id``).
            selector_nth: 0-based index to disambiguate when a selector
                matches more than one visible target.
            selector_x: CSS-pixel x (``selector_type="visual"`` only).
            selector_y: CSS-pixel y (``selector_type="visual"`` only) —
                together with ``selector_x``, resolves to an exact 1x1 box.
            viewport_preset: Named device (see
                :func:`voidcrawl.viewport.list_device_presets`). Mutually
                exclusive with ``viewport_width``/``viewport_height``.
                One-shot: restores whatever viewport was active before,
                even on error.
            viewport_width: Custom one-shot viewport width in CSS pixels.
                Requires ``viewport_height``.
            viewport_height: Custom one-shot viewport height in CSS pixels.
                Requires ``viewport_width``.
            viewport_device_scale_factor: DPR for a custom viewport
                (default ``1.0``). Ignored with ``viewport_preset``.
            viewport_mobile: Emulate a mobile viewport for a custom size —
                also enables touch (default ``False``). Ignored with
                ``viewport_preset``.
            scroll_viewports: Scroll to N viewport-heights from the top
                before capturing (``2.0`` = "scrolled down twice"). Mutually
                exclusive with ``scroll_pixels``. Restored after capture.
            scroll_pixels: Scroll to an absolute pixel Y before capturing.
            full_page: Capture the full scrollable page (default ``True``).
                ``False`` captures only the visible viewport. Ignored when
                ``bbox``/``selector_type`` is set.
        """
        ...
    async def set_viewport(
        self,
        preset: str | None = None,
        width: int | None = None,
        height: int | None = None,
        device_scale_factor: float | None = None,
        mobile: bool | None = None,
    ) -> None:
        """Persistently override this page's CDP viewport — dimensions, DPR,
        mobile/touch identity, and (for a preset) a matching UA. Stays in
        effect until :meth:`clear_viewport` or another `set_viewport` call.
        Pass either ``preset`` or ``width``+``height``."""
        ...
    async def clear_viewport(self) -> None:
        """Clear a :meth:`set_viewport` override, returning to the
        session's launch-time default viewport."""
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
    async def click_by_role(
        self, role: str, name: str, nth: int = 0, humanize: bool = False
    ) -> None:
        """Click the *nth* element matching accessibility ``role`` + ``name``.

        Markup-independent analogue of ``click_element``: resolves via the AX
        tree, bridges to the DOM, scrolls into view, and clicks. Raises if no
        such node exists.

        Args:
            role: Computed accessibility role, e.g. ``"button"``, ``"link"``.
            name: Computed accessible name (exact match).
            nth: 0-based index when several nodes match.
            humanize: Click at the element's box-model centre with a humanized
                compositor pointer path (curved, min-jerk, tremor) instead of a
                DOM ``.click()``. Off by default.
        """
        ...
    async def move_mouse(self, x: float, y: float, humanize: bool = False) -> None:
        """Move the virtual cursor to ``(x, y)`` via CDP ``Input.dispatchMouseEvent``.

        With ``humanize=True`` the cursor travels a realistic curved, minimum-jerk,
        lightly-tremored path (multiple ``MouseMoved`` events) from its last
        position; otherwise it jumps in one event. No page-world JS is injected."""
        ...
    async def click_xy(self, x: float, y: float, humanize: bool = False) -> None:
        """Click at ``(x, y)`` with a trusted compositor event (press → release).

        With ``humanize=True`` the cursor first travels a human-like path there
        (see :meth:`move_mouse`). The programmatic analogue of the
        ``click_visual_coords`` MCP tool."""
        ...
    async def click_ax_in_frame(
        self,
        frame_url_pattern: str,
        role: str,
        name: str,
        nth: int = 0,
        humanize: bool = False,
    ) -> None:
        """Click an element by AX ``role`` + ``name`` inside a specific frame.

        The cross-frame, shadow-piercing analogue of :meth:`click_by_role`:
        roots the AX tree at the frame matched by ``frame_url_pattern`` and
        descends into closed shadow roots, then clicks the match at its
        box-model centre with a real **compositor** mouse event (a trusted
        click, unlike a DOM ``.click()``). Reaches widgets the page's own JS
        cannot — e.g. Cloudflare Turnstile's "Verify you are human" checkbox in
        a closed shadow root inside a cross-origin ``challenges.cloudflare.com``
        iframe. Empty ``name`` matches any node of that ``role``.

        Cross-origin google.com / cloudflare frames must be in-process: launch
        the session with ``extra_args=["disable-site-isolation-trials"]``.
        """
        ...
    async def ax_box_in_frame(
        self, frame_url_pattern: str, role: str, name: str, nth: int = 0
    ) -> list[float]:
        """Locate an AX ``role`` + ``name`` inside a frame; return its on-page
        rectangle ``[x, y, width, height]`` in CSS pixels.

        Same cross-frame, closed-shadow-piercing resolution as
        :meth:`click_ax_in_frame`, but returns the geometry instead of clicking
        — so you can drive a **humanized** click yourself (curved approach via
        :meth:`dispatch_mouse_event`, press at a jittered point in the box).
        Empty ``name`` matches any node of that ``role``."""
        ...
    async def ax_outline_in_frame(
        self, frame_url_pattern: str, depth: int | None = None
    ) -> str:
        """Compact accessibility outline of a specific (possibly cross-origin)
        frame — pierces closed shadow roots. Discover the role / accessible name
        to pass to :meth:`click_ax_in_frame`."""
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

class InterruptInfo:
    interrupt_id: str
    target_id: str
    code: str
    summary: str
    state: str
    expires_in_ms: int

class ContextCleanupReport:
    state_binding: str
    disposal_state: str
    cleanup_complete: bool

class IsolatedBrowserContext:
    state_binding: str
    def page(self) -> Page: ...
    async def dispose(self) -> ContextCleanupReport: ...
    async def __aenter__(self) -> IsolatedBrowserContext: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> bool: ...

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
        user_data_dir: str | None = None,
        port: int | None = None,
        cdp_mode: Literal["normal", "minimal"] | None = None,
    ) -> None: ...
    async def launch(self) -> None: ...
    async def new_page(self, url: str | None = None) -> Page: ...
    async def new_isolated_context(self) -> IsolatedBrowserContext: ...
    async def state_binding(self) -> str: ...
    async def new_page_in_window(self, url: str) -> Page: ...
    async def attach_page(self, target_id: str) -> Page: ...
    async def interrupt(
        self, page: Page, code: str, summary: str, ttl_seconds: int = 600
    ) -> InterruptInfo: ...
    async def resume(self, interrupt_id: str) -> InterruptInfo: ...
    async def release(self, interrupt_id: str) -> InterruptInfo: ...
    async def websocket_url(self) -> str: ...
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
def py_profile_registry_root(root: str | None = None) -> str: ...
def py_profile_registry_list(root: str | None = None) -> str: ...
def py_profile_registry_create(
    id: str,  # noqa: A002
    description: str | None = None,
    labels: list[str] | None = None,
    root: str | None = None,
) -> str: ...
def py_profile_registry_describe(
    id: str,  # noqa: A002
    root: str | None = None,
) -> str: ...
def py_profile_registry_clone(
    source_id_or_path: str,
    id: str,  # noqa: A002
    description: str | None = None,
    labels: list[str] | None = None,
    root: str | None = None,
) -> str: ...
def py_profile_registry_snapshot(
    id: str,  # noqa: A002
    root: str | None = None,
) -> ManagedProfileSnapshot: ...
def py_profile_registry_split(
    id: str,  # noqa: A002
    copies: int = 2,
    root: str | None = None,
) -> ManagedProfileSplit: ...
def py_profile_registry_fork(
    source: str = "Default",
    copies: int = 2,
    root: str | None = None,
) -> ManagedProfileSplit: ...
def py_profile_registry_delete(
    id: str,  # noqa: A002
    root: str | None = None,
) -> bool: ...
def py_profile_pool_list(root: str | None = None) -> str: ...
def py_profile_pool_create(
    name: str,
    profile_ids: list[str],
    max_active: int = 3,
    root: str | None = None,
) -> str: ...
def py_profile_pool_describe(name: str, root: str | None = None) -> str: ...
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

def list_device_presets() -> list[tuple[str, int, int, float, bool]]:
    """List named device presets as ``(name, width, height,
    device_scale_factor, mobile)`` tuples. See
    :func:`voidcrawl.viewport.list_device_presets` for the validated
    Python-facing wrapper."""
    ...

# ── Exceptions ──────────────────────────────────────────────────────────
# ruff: noqa: N818  — these are the public exception names, preserved for API compat

class VoidCrawlError(Exception):
    """Base class for native errors with stable, secret-safe dispatch fields."""

    code: str
    category: str

class ManagedProfileSnapshot:
    path: str
    async def close(self) -> None: ...
    async def __aenter__(self) -> ManagedProfileSnapshot: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> None: ...

class ManagedProfileSplit:
    source_id: str
    paths: list[str]
    def __len__(self) -> int: ...
    async def close(self) -> None: ...
    async def __aenter__(self) -> ManagedProfileSplit: ...
    async def __aexit__(
        self, exc_type: object = None, exc_val: object = None, exc_tb: object = None
    ) -> None: ...

class NavigationError(VoidCrawlError): ...

class NavigationTimeoutError(NavigationError):
    url: str
    wait_phase: str
    timeout: float
    elapsed: float

class BrowserClosedError(NavigationError): ...
class ResponseTimeoutError(VoidCrawlError): ...

class SessionInterrupted(VoidCrawlError):
    interrupt_id: str

class InterruptExpired(VoidCrawlError): ...
class InterruptTerminal(VoidCrawlError): ...
class InterruptNotFound(VoidCrawlError): ...

class ChromeProfileBusy(VoidCrawlError):
    """Chrome's own SingletonLock prevented profile launch."""

class ProfileBusy(VoidCrawlError):
    """Another voidcrawl process holds the profile lock (non-blocking acquire)."""

    profile: str
    owner_pid: int | None
    acquired_at: int | None

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
