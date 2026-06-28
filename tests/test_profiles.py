"""Tests for voidcrawl.profiles — does NOT launch Chrome.

The underlying Rust layer is tested in crates/core/tests/profile_lock.rs.
Here we verify the Python surface: exception classes, type exports,
and that acquire_profile raises ProfileNotFound for unknown names.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, cast

import pytest

if TYPE_CHECKING:
    from pathlib import Path

from voidcrawl import (
    CaptchaDetected,
    ProfileBusy,
    ProfileLeaseExpired,
    ProfileNotFound,
    ProfileRegistry,
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


def test_managed_profile_registry_crud_and_pool(tmp_path: Path) -> None:
    registry = ProfileRegistry(str(tmp_path))

    created = registry.create_profile(
        "google-001",
        description="SERP login",
        labels=("serp", "google"),
    )

    assert created["id"] == "google-001"
    assert created["description"] == "SERP login"
    assert created["labels"] == ["serp", "google"]
    assert created["status"] == "available"

    listed = registry.list_profiles()
    assert [profile["id"] for profile in listed] == ["google-001"]

    pool = registry.create_pool("google-serp", ["google-001"])
    assert pool["name"] == "google-serp"
    assert pool["profile_ids"] == ["google-001"]
    assert pool["max_active"] == 3

    resolved = registry.resolve_pool("google-serp")
    profiles = cast("list[dict[str, object]]", resolved["profiles"])
    assert profiles[0]["id"] == "google-001"

    assert registry.delete_profile("google-001") is True
    assert registry.list_profiles() == []
