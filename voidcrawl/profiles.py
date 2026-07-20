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

import json
import os
import shutil
import time
from contextlib import asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

from voidcrawl._ext import (
    CaptchaDetected,
    ChromeProfileBusy,
    ManagedProfileSnapshot,
    ManagedProfileSplit,
    ProfileBusy,
    ProfileHandle,
    ProfileLeaseExpired,
    ProfileNotFound,
    VoidCrawlError,
    py_acquire_profile,
    py_list_profiles,
)

try:
    from voidcrawl._ext import (
        py_profile_pool_create,
        py_profile_pool_describe,
        py_profile_pool_list,
        py_profile_registry_clone,
        py_profile_registry_create,
        py_profile_registry_delete,
        py_profile_registry_describe,
        py_profile_registry_fork,
        py_profile_registry_list,
        py_profile_registry_root,
        py_profile_registry_snapshot,
        py_profile_registry_split,
    )
except (
    ImportError
):  # pragma: no cover - only hit before the local PyO3 extension is rebuilt
    py_profile_pool_create = None  # type: ignore[assignment]
    py_profile_pool_describe = None  # type: ignore[assignment]
    py_profile_pool_list = None  # type: ignore[assignment]
    py_profile_registry_clone = None  # type: ignore[assignment]
    py_profile_registry_create = None  # type: ignore[assignment]
    py_profile_registry_delete = None  # type: ignore[assignment]
    py_profile_registry_describe = None  # type: ignore[assignment]
    py_profile_registry_fork = None  # type: ignore[assignment]
    py_profile_registry_list = None  # type: ignore[assignment]
    py_profile_registry_root = None  # type: ignore[assignment]
    py_profile_registry_snapshot = None  # type: ignore[assignment]
    py_profile_registry_split = None  # type: ignore[assignment]

JsonDict = dict[str, Any]
RegistryManifest = dict[str, dict[str, JsonDict]]

__all__ = [
    "CaptchaDetected",
    "ChromeProfileBusy",
    "ManagedProfileSnapshot",
    "ManagedProfileSplit",
    "ProfileBusy",
    "ProfileHandle",
    "ProfileLeaseExpired",
    "ProfileNotFound",
    "ProfileRegistry",
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


class ProfileRegistry:
    """VoidCrawl-managed standalone Chromium profile registry.

    Profiles live under ``$VOIDCRAWL_PROFILE_ROOT`` or the platform data-dir
    default, and each profile path is a standalone Chrome ``user_data_dir``.
    Methods return metadata only; cookie and localStorage values are never read.
    """

    def __init__(self, root: str | None = None) -> None:
        if py_profile_registry_root is not None:
            self.root = py_profile_registry_root(root)
        else:
            self.root = str(_fallback_root(root))

    @classmethod
    def default(cls) -> ProfileRegistry:
        return cls()

    def create_profile(
        self,
        id: str,  # noqa: A002 - public API mirrors profile registry schema.
        *,
        description: str | None = None,
        labels: tuple[str, ...] | list[str] = (),
    ) -> dict[str, object]:
        if py_profile_registry_create is not None:
            return cast(
                "JsonDict",
                json.loads(
                    py_profile_registry_create(id, description, list(labels), self.root)
                ),
            )
        return _fallback_create_profile(self.root, id, description, list(labels))

    def clone_profile(
        self,
        source_id_or_path: str,
        id: str,  # noqa: A002 - public API mirrors profile registry schema.
        *,
        description: str | None = None,
        labels: tuple[str, ...] | list[str] = (),
    ) -> dict[str, object]:
        if py_profile_registry_clone is not None:
            return cast(
                "JsonDict",
                json.loads(
                    py_profile_registry_clone(
                        source_id_or_path,
                        id,
                        description,
                        list(labels),
                        self.root,
                    )
                ),
            )
        return _fallback_clone_profile(
            self.root, source_id_or_path, id, description, list(labels)
        )

    def snapshot_profile(
        self,
        id: str,  # noqa: A002 - public API mirrors profile registry schema.
    ) -> ManagedProfileSnapshot:
        """Create a quiesced, temporary clone that cleans itself up.

        Use the returned object as an async context manager. The source's OS
        lease is held while copying; lock and Chrome Singleton files are
        excluded from the snapshot.
        """
        if py_profile_registry_snapshot is None:
            raise RuntimeError("managed profile snapshots require the native extension")
        return py_profile_registry_snapshot(id, self.root)

    def fork_profile(
        self,
        source: str = "Default",
        *,
        copies: int = 2,
    ) -> ManagedProfileSplit:
        """Fork a closed native Chrome profile into concurrent instances.

        ``source`` may be a discovered profile name such as ``"Default"`` or
        an explicit native profile-directory path. The source Chrome must be
        closed so its cookie databases and storage are quiescent. Each result
        is a temporary standalone ``user_data_dir`` containing the selected
        profile as ``Default`` plus Chrome's root ``Local State`` metadata.

        The copies start with the same login state and profile identity, then
        intentionally diverge. They are deleted when the async context exits.
        """
        if py_profile_registry_fork is None:
            raise RuntimeError("native profile forking requires the native extension")
        return py_profile_registry_fork(source, copies, self.root)

    def split_profile(
        self,
        id: str,  # noqa: A002 - public API mirrors profile registry schema.
        *,
        copies: int = 2,
    ) -> ManagedProfileSplit:
        """Prepare isolated copies of one profile for concurrent Chrome instances.

        The returned async context manager performs the copy work off the
        asyncio thread. It takes one source lease across the complete split, so
        every copy starts from the same quiesced baseline. Each path is a
        separate Chrome ``user_data_dir`` and can therefore run concurrently
        without a ``SingletonLock`` conflict.

        This is copy-on-start isolation, not live synchronization: cookies,
        storage, and other writes diverge once the Chrome instances launch and
        are not merged back into the source.

        Example::

            async with registry.split_profile("work", copies=2) as split:
                first_path, second_path = split.paths
                # Launch one BrowserSession per path.
        """
        if py_profile_registry_split is None:
            raise RuntimeError(
                "managed profile splitting requires the native extension"
            )
        return py_profile_registry_split(id, copies, self.root)

    def list_profiles(self) -> list[dict[str, object]]:
        if py_profile_registry_list is not None:
            return cast(
                "list[JsonDict]", json.loads(py_profile_registry_list(self.root))
            )
        return list(_fallback_manifest(self.root)["profiles"].values())

    def describe_profile(
        self,
        id: str,  # noqa: A002 - public API mirrors profile registry schema.
    ) -> dict[str, object]:
        if py_profile_registry_describe is not None:
            return cast(
                "JsonDict", json.loads(py_profile_registry_describe(id, self.root))
            )
        manifest = _fallback_manifest(self.root)
        try:
            return manifest["profiles"][id]
        except KeyError as exc:
            raise ProfileNotFound(f"profile not found: {id}") from exc

    def delete_profile(
        self,
        id: str,  # noqa: A002 - public API mirrors profile registry schema.
    ) -> bool:
        if py_profile_registry_delete is not None:
            return py_profile_registry_delete(id, self.root)
        return _fallback_delete_profile(self.root, id)

    def create_pool(
        self,
        name: str,
        profile_ids: list[str] | tuple[str, ...],
        *,
        max_active: int = 3,
    ) -> dict[str, object]:
        if py_profile_pool_create is not None:
            return cast(
                "JsonDict",
                json.loads(
                    py_profile_pool_create(
                        name, list(profile_ids), max_active, self.root
                    )
                ),
            )
        return _fallback_create_pool(self.root, name, list(profile_ids), max_active)

    def list_pools(self) -> list[dict[str, object]]:
        if py_profile_pool_list is not None:
            return cast("list[JsonDict]", json.loads(py_profile_pool_list(self.root)))
        return list(_fallback_manifest(self.root)["pools"].values())

    def resolve_pool(self, name: str) -> dict[str, object]:
        if py_profile_pool_describe is not None:
            return cast(
                "JsonDict", json.loads(py_profile_pool_describe(name, self.root))
            )
        manifest = _fallback_manifest(self.root)
        pool = manifest["pools"][name]
        profile_ids = cast("list[str]", pool["profile_ids"])
        return {
            "pool": pool,
            "profiles": [
                manifest["profiles"][profile_id] for profile_id in profile_ids
            ],
        }


def _fallback_root(root: str | None) -> Path:
    if root:
        return Path(root).expanduser()
    env_root = os.environ.get("VOIDCRAWL_PROFILE_ROOT")
    if env_root:
        return Path(env_root).expanduser()
    data_home = os.environ.get("XDG_DATA_HOME")
    if data_home:
        return Path(data_home) / "voidcrawl" / "profiles"
    return Path.home() / ".local" / "share" / "voidcrawl" / "profiles"


def _fallback_manifest_path(root: str | Path) -> Path:
    return Path(root) / "registry.json"


def _fallback_manifest(root: str | Path) -> RegistryManifest:
    path = _fallback_manifest_path(root)
    if not path.exists():
        return {"profiles": {}, "pools": {}}
    return cast("RegistryManifest", json.loads(path.read_text(encoding="utf-8")))


def _fallback_write_manifest(root: str | Path, manifest: RegistryManifest) -> None:
    root_path = Path(root)
    root_path.mkdir(parents=True, exist_ok=True)
    _fallback_manifest_path(root_path).write_text(
        json.dumps(manifest, indent=2), encoding="utf-8"
    )


def _fallback_profile_description(
    root: str | Path,
    profile_id: str,
    *,
    description: str | None,
    labels: list[str],
) -> dict[str, object]:
    path = Path(root) / profile_id
    return {
        "id": profile_id,
        "path": str(path),
        "created_at": int(time.time()),
        "last_used_at": None,
        "labels": labels,
        "description": description,
        "size": 2,
        "status": "available",
    }


def _fallback_create_profile(
    root: str | Path,
    profile_id: str,
    description: str | None,
    labels: list[str],
) -> dict[str, object]:
    manifest = _fallback_manifest(root)
    if profile_id in manifest["profiles"]:
        raise VoidCrawlError(f"managed profile already exists: {profile_id}")
    default_dir = Path(root) / profile_id / "Default"
    default_dir.mkdir(parents=True, exist_ok=True)
    (default_dir / "Preferences").write_text("{}", encoding="utf-8")
    record = _fallback_profile_description(
        root, profile_id, description=description, labels=labels
    )
    manifest["profiles"][profile_id] = record
    _fallback_write_manifest(root, manifest)
    return record


def _fallback_clone_profile(
    root: str | Path,
    source_id_or_path: str,
    profile_id: str,
    description: str | None,
    labels: list[str],
) -> dict[str, object]:
    manifest = _fallback_manifest(root)
    source_record = manifest["profiles"].get(source_id_or_path)
    source = (
        Path(str(source_record["path"]))
        if source_record is not None
        else Path(source_id_or_path).expanduser()
    )
    if not source.is_dir():
        raise ProfileNotFound(f"profile not found: {source_id_or_path}")
    destination = Path(root) / profile_id
    shutil.copytree(source, destination)
    record = _fallback_profile_description(
        root, profile_id, description=description, labels=labels
    )
    manifest["profiles"][profile_id] = record
    _fallback_write_manifest(root, manifest)
    return record


def _fallback_delete_profile(root: str | Path, profile_id: str) -> bool:
    manifest = _fallback_manifest(root)
    record = manifest["profiles"].pop(profile_id, None)
    if record is None:
        return False
    shutil.rmtree(str(record["path"]), ignore_errors=True)
    for pool in manifest["pools"].values():
        profile_ids = cast("list[str]", pool["profile_ids"])
        pool["profile_ids"] = [pid for pid in profile_ids if pid != profile_id]
    _fallback_write_manifest(root, manifest)
    return True


def _fallback_create_pool(
    root: str | Path,
    name: str,
    profile_ids: list[str],
    max_active: int,
) -> dict[str, object]:
    manifest = _fallback_manifest(root)
    missing = [
        profile_id
        for profile_id in profile_ids
        if profile_id not in manifest["profiles"]
    ]
    if missing:
        raise ProfileNotFound(f"profile not found: {missing[0]}")
    pool = {
        "name": name,
        "profile_ids": profile_ids,
        "max_active": max(1, max_active),
        "round_robin": True,
    }
    manifest["pools"][name] = pool
    _fallback_write_manifest(root, manifest)
    return pool


async def acquire_profile(
    name: str,
    lease_timeout: float = 300.0,
    *,
    headless: bool = True,
) -> ProfileHandle:
    """Acquire an exclusive lease on a Chrome profile.

    Args:
        name: Profile directory name as Chrome stores it (e.g.
            ``"Default"``, ``"Profile 1"``).
        lease_timeout: Seconds to poll for the lock before giving up.
            ``0`` means fail immediately if busy.
        headless: Run Chrome in headless mode (default). Set ``False``
            for a visible window — useful for a one-time manual login
            before the profile is used for scraping.

    Raises:
        ProfileBusy: Another voidcrawl process holds the lock and the
            timeout is zero.
        ProfileLeaseExpired: Timed out waiting for the lock.
        ProfileNotFound: No matching profile directory in the platform
            default dirs.
    """
    if not any(profile_name == name for profile_name, _ in list_profiles()):
        raise ProfileNotFound(f"profile not found: {name}")

    return await py_acquire_profile(name, lease_timeout, headless)


@asynccontextmanager
async def with_profile(
    name: str,
    lease_timeout: float = 300.0,
    *,
    headless: bool = True,
) -> AsyncIterator[ProfileHandle]:
    """Async context manager: acquire, yield, release.

    Example::

        async with with_profile("Default") as handle:
            page = await handle.new_page("https://linkedin.com/in/me")
            html = await page.content()

    Pass ``headless=False`` to see the Chrome window (e.g. for a
    manual login flow).
    """
    handle = await acquire_profile(name, lease_timeout, headless=headless)
    try:
        yield handle
    finally:
        await handle.release()
