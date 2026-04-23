"""Native Chrome profile leasing.

Use this when you need to reuse an existing logged-in Chrome profile
(cookies, local storage, extensions) for scraping. Profile leases are
exclusive across voidcrawl processes via a ``.voidcrawl.lock`` file
inside the profile directory.

Example::

    from voidcrawl.profiles import with_profile

    async with with_profile("Default") as handle:
        page = await handle.new_page("https://example.com")
        html = await page.content()
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

from voidcrawl._ext import (
    CaptchaDetected,
    ProfileBusy,
    ProfileHandle,
    ProfileLeaseExpired,
    ProfileNotFound,
    VoidCrawlError,
    py_acquire_profile,
    py_list_profiles,
)

__all__ = [
    "CaptchaDetected",
    "ProfileBusy",
    "ProfileHandle",
    "ProfileLeaseExpired",
    "ProfileNotFound",
    "VoidCrawlError",
    "acquire_profile",
    "list_profiles",
    "with_profile",
]


def list_profiles() -> list[tuple[str, str]]:
    """Return ``[(name, path), ...]`` for every Chrome profile discovered.

    Searches platform default dirs:
        * Linux: ``~/.config/google-chrome``, ``~/.config/chromium``
        * macOS: ``~/Library/Application Support/Google/Chrome``
        * Windows: ``%LOCALAPPDATA%\\Google\\Chrome\\User Data``

    Only directories containing a ``Preferences`` file are returned.
    """
    return py_list_profiles()


async def acquire_profile(name: str, lease_timeout: float = 300.0) -> ProfileHandle:
    """Acquire an exclusive lease on a Chrome profile.

    Args:
        name: Profile directory name as Chrome stores it (e.g.
            ``"Default"``, ``"Profile 1"``).
        lease_timeout: Seconds to poll for the lock before giving up.
            ``0`` means fail immediately if busy.

    Raises:
        ProfileBusy: Another voidcrawl process holds the lock and the
            timeout is zero.
        ProfileLeaseExpired: Timed out waiting for the lock.
        ProfileNotFound: No matching profile directory in the platform
            default dirs.
    """
    return await py_acquire_profile(name, lease_timeout)


@asynccontextmanager
async def with_profile(
    name: str, lease_timeout: float = 300.0
) -> AsyncIterator[ProfileHandle]:
    """Async context manager: acquire, yield, release.

    Example::

        async with with_profile("Default") as handle:
            page = await handle.new_page("https://linkedin.com/in/me")
            html = await page.content()
    """
    handle = await acquire_profile(name, lease_timeout)
    try:
        yield handle
    finally:
        await handle.release()
