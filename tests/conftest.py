"""Shared fixtures for VoidCrawl tests."""

from __future__ import annotations

import pytest

from voidcrawl.scale import ResourceSnapshot

# ── Resource snapshot fixtures ───────────────────────────────────────────


def _make_snapshot(**overrides: int | float | bool | None) -> ResourceSnapshot:
    """Build a ResourceSnapshot with sensible defaults, overridden by kwargs."""
    defaults: dict[str, int | float | bool | None] = {
        "free_ram_mb": 8000,
        "total_ram_mb": 16000,
        "cpu_cores": 4,
        "load_avg_1m": 1.0,
        "swap_used_mb": 0,
        "fd_soft_limit": 65536,
        "has_display": False,
        "in_container": False,
        "cgroup_mem_limit_mb": None,
    }
    defaults.update(overrides)
    return ResourceSnapshot(**defaults)  # type: ignore[arg-type]


@pytest.fixture
def snapshot_server() -> ResourceSnapshot:
    """A beefy server: 32 GB RAM, 16 cores, no display, not in container."""
    return _make_snapshot(
        free_ram_mb=28_000,
        total_ram_mb=32_000,
        cpu_cores=16,
    )


@pytest.fixture
def snapshot_laptop() -> ResourceSnapshot:
    """A typical developer laptop: 16 GB, 8 cores, display present."""
    return _make_snapshot(
        free_ram_mb=8_000,
        total_ram_mb=16_000,
        cpu_cores=8,
        has_display=True,
    )


@pytest.fixture
def snapshot_embedded() -> ResourceSnapshot:
    """Raspberry Pi / CI runner: 1 GB, 2 cores, no display."""
    return _make_snapshot(
        free_ram_mb=800,
        total_ram_mb=1024,
        cpu_cores=2,
        fd_soft_limit=1024,
    )


@pytest.fixture
def snapshot_container_cgroup() -> ResourceSnapshot:
    """Docker container with cgroup memory limit."""
    return _make_snapshot(
        free_ram_mb=4000,
        total_ram_mb=8000,
        cpu_cores=4,
        in_container=True,
        cgroup_mem_limit_mb=2048,
    )


@pytest.fixture
def snapshot_low_ram() -> ResourceSnapshot:
    """Below Chrome's 600 MB minimum — should trigger InsufficientResourcesError."""
    return _make_snapshot(free_ram_mb=400, total_ram_mb=512)


@pytest.fixture
def snapshot_low_fd() -> ResourceSnapshot:
    """File descriptor limit below 256 — should trigger InsufficientResourcesError."""
    return _make_snapshot(fd_soft_limit=128)


@pytest.fixture
def snapshot_swapping_server() -> ResourceSnapshot:
    """Server with active swap — should downgrade advanced -> balanced."""
    return _make_snapshot(
        free_ram_mb=16_000,
        total_ram_mb=32_000,
        cpu_cores=8,
        swap_used_mb=500,
    )


@pytest.fixture
def snapshot_high_load() -> ResourceSnapshot:
    """Machine under heavy CPU load (> 1.5x cores)."""
    return _make_snapshot(
        free_ram_mb=8000,
        total_ram_mb=16000,
        cpu_cores=4,
        load_avg_1m=8.0,
    )
