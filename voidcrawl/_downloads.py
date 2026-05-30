"""Ergonomic context manager for action-triggered downloads.

Brackets the ``arm_download`` / ``wait_download`` primitives so the triggering
action (a click) reads naturally between them — the Playwright
``expect_download`` shape::

    async with capture_download(tab, "/tmp/dl", timeout=90) as dl:
        await tab.click_by_role("button", "Download")
    outcome = dl.value  # DownloadOutcome

``tab`` is a :class:`~voidcrawl.Page` or :class:`~voidcrawl.PooledTab`.
"""

from __future__ import annotations

import contextlib
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from voidcrawl._ext import DownloadCapture, DownloadOutcome

    class _Downloadable(Protocol):
        async def arm_download(
            self,
            dir: str,  # noqa: A002 — mirrors the native binding's parameter name
            max_bytes: int | None = None,
        ) -> DownloadCapture: ...
        async def wait_download(
            self, capture: DownloadCapture, timeout: float = 120.0
        ) -> DownloadOutcome: ...
        async def reset_download(self) -> None: ...


class capture_download:  # noqa: N801 — context-manager factory, lowercase by convention
    """Async context manager that arms a download, lets you trigger it inside
    the block, then awaits it on a clean exit.

    Args:
        tab: A :class:`~voidcrawl.Page` or :class:`~voidcrawl.PooledTab`.
        dir: Directory the file is saved into (treat as quarantine).
        max_bytes: Reject downloads larger than this (default 100 MiB).
        timeout: Seconds to wait for the download after the block exits.

    After the ``async with`` block, :attr:`value` holds the
    :class:`~voidcrawl.DownloadOutcome`. If the block raises, the download is not
    awaited and the exception propagates.
    """

    def __init__(
        self,
        tab: _Downloadable,
        dir: str,  # noqa: A002 — mirrors the native binding's parameter name
        *,
        max_bytes: int | None = None,
        timeout: float = 120.0,
    ) -> None:
        self._tab = tab
        self._dir = dir
        self._max_bytes = max_bytes
        self._timeout = timeout
        self._capture: DownloadCapture | None = None
        self._outcome: DownloadOutcome | None = None

    async def __aenter__(self) -> capture_download:
        self._outcome = None  # clear any stale result from a prior use
        self._capture = await self._tab.arm_download(self._dir, self._max_bytes)
        return self

    async def __aexit__(
        self, exc_type: object, exc_val: object, exc_tb: object
    ) -> bool:
        if exc_type is None and self._capture is not None:
            # Clean exit: collect the download (wait_download resets behavior).
            self._outcome = await self._tab.wait_download(self._capture, self._timeout)
        else:
            # Error inside the block: release the armed download behavior so a
            # pooled tab doesn't return to the pool still in allowAndName mode.
            with contextlib.suppress(Exception):
                await self._tab.reset_download()
        return False

    @property
    def value(self) -> DownloadOutcome:
        """The captured :class:`~voidcrawl.DownloadOutcome` (after the block)."""
        if self._outcome is None:
            msg = (
                "download not captured yet — read `.value` after the `async with` block"
            )
            raise RuntimeError(msg)
        return self._outcome
