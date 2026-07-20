"""Tests for voidcrawl.profiles — does NOT launch Chrome.

The underlying Rust layer is tested in crates/core/tests/profile_lock.rs.
Here we verify the Python surface: exception classes, type exports,
and that acquire_profile raises ProfileNotFound for unknown names.
"""

from __future__ import annotations

from pathlib import Path
from typing import cast

import pytest

from voidcrawl import (
    CaptchaDetected,
    ChromeProfileBusy,
    ManagedProfileSplit,
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
    assert issubclass(ChromeProfileBusy, VoidCrawlError)
    assert ManagedProfileSplit.__name__ == "ManagedProfileSplit"
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


@pytest.mark.asyncio
async def test_managed_profile_snapshot_is_temporary(tmp_path: Path) -> None:
    registry = ProfileRegistry(str(tmp_path))
    created = registry.create_profile("source")
    source = tmp_path / "source" / "Default"
    (source / "Cookies").write_text("cookie-data")

    async with registry.snapshot_profile("source") as snapshot:
        snapshot_path = snapshot.path
        assert (tmp_path / "source").exists()
        cookies = Path(snapshot_path) / "Default" / "Cookies"
        assert cookies.read_text() == "cookie-data"
        assert not (Path(snapshot_path) / ".voidcrawl.lock").exists()

    assert not Path(snapshot_path).exists()
    assert created["id"] == "source"


@pytest.mark.asyncio
async def test_managed_profile_split_has_one_baseline_and_unique_cleanup(
    tmp_path: Path,
) -> None:
    registry = ProfileRegistry(str(tmp_path))
    registry.create_profile("source")
    source_cookie = tmp_path / "source" / "Default" / "Cookies"
    source_cookie.write_text("shared-baseline")

    split = registry.split_profile("source", copies=2)
    assert len(split) == 2
    with pytest.raises(RuntimeError, match="only inside"):
        _ = split.paths

    async with split:
        paths = [Path(path) for path in split.paths]
        assert len(set(paths)) == 2
        assert all(
            (path / "Default" / "Cookies").read_text() == "shared-baseline"
            for path in paths
        )

    assert all(not path.exists() for path in paths)


@pytest.mark.asyncio
async def test_native_profile_fork_normalizes_default_profile(tmp_path: Path) -> None:
    user_data = tmp_path / "native"
    native_profile = user_data / "Profile 3"
    native_profile.mkdir(parents=True)
    (native_profile / "Preferences").write_text('{"profile":{"name":"Work"}}')
    (native_profile / "Cookies").write_text("authenticated-state")
    (user_data / "Local State").write_text("encryption-context")
    registry = ProfileRegistry(str(tmp_path / "registry"))

    async with registry.fork_profile(str(native_profile), copies=2) as fork:
        paths = [Path(path) for path in fork.paths]
        assert all(
            (path / "Default" / "Cookies").read_text() == "authenticated-state"
            for path in paths
        )
        assert all(
            (path / "Local State").read_text() == "encryption-context" for path in paths
        )

    assert all(not path.exists() for path in paths)


@pytest.mark.asyncio
async def test_native_profile_fork_rejects_running_chrome_root(tmp_path: Path) -> None:
    user_data = tmp_path / "native"
    native_profile = user_data / "Default"
    native_profile.mkdir(parents=True)
    (native_profile / "Preferences").write_text("{}")
    (user_data / "SingletonLock").write_text("active")
    registry = ProfileRegistry(str(tmp_path / "registry"))

    with pytest.raises(ChromeProfileBusy):
        async with registry.fork_profile(str(native_profile), copies=2):
            pass


def test_managed_profile_split_validates_copy_count(tmp_path: Path) -> None:
    registry = ProfileRegistry(str(tmp_path))
    registry.create_profile("source")

    with pytest.raises(ValueError, match="between 2 and 16"):
        registry.split_profile("source", copies=1)
