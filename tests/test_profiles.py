"""Tests for voidcrawl.profiles — does NOT launch Chrome.

The underlying Rust layer is tested in crates/core/tests/profile_lock.rs.
Here we verify the Python surface: exception classes, type exports,
and that acquire_profile raises ProfileNotFound for unknown names.
"""

from __future__ import annotations

import pytest

from voidcrawl import (
    CaptchaDetected,
    ProfileBusy,
    ProfileLeaseExpired,
    ProfileNotFound,
    VoidCrawlError,
    acquire_profile,
    list_profiles,
)


def test_list_profiles_returns_list_of_tuples() -> None:
    profiles = list_profiles()
    assert isinstance(profiles, list)
    for entry in profiles:
        assert isinstance(entry, tuple)
        assert len(entry) == 2
        name, path = entry
        assert isinstance(name, str)
        assert isinstance(path, str)


def test_exception_hierarchy() -> None:
    assert issubclass(ProfileBusy, VoidCrawlError)
    assert issubclass(ProfileLeaseExpired, VoidCrawlError)
    assert issubclass(ProfileNotFound, VoidCrawlError)
    assert issubclass(CaptchaDetected, VoidCrawlError)


@pytest.mark.asyncio
async def test_acquire_unknown_profile_raises_not_found() -> None:
    with pytest.raises(ProfileNotFound):
        await acquire_profile("NoSuchProfileXYZ_9999", lease_timeout=0.0)
