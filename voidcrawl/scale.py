"""Resource-aware scale profiles for VoidCrawl pool configuration.

Three profiles match common deployment targets:

* ``"minimal"``   — embedded devices, CI runners, Raspberry Pi
* ``"balanced"``  — developer laptops / desktop PCs  *(default)*
* ``"advanced"``  — dedicated servers (p95 of measured capacity)

The module reads system resources from ``/proc`` and cgroup files with
no third-party dependencies, then computes a safe :class:`~voidcrawl.PoolConfig`
via :func:`compute_scale`.

Scripted mode::

    python -m voidcrawl.scale                      # auto-detect from $SCALE_PROFILE
    python -m voidcrawl.scale --profile advanced   # force a profile
    python -m voidcrawl.scale --json               # machine-readable JSON output

Docker entrypoint usage::

    report = compute_scale(profile=os.environ.get("SCALE_PROFILE", "balanced"))
    supervisord_conf = generate_supervisord_conf(report)
"""

from __future__ import annotations

import contextlib
import math
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Literal

try:
    from rich import print as rprint  # type: ignore[import-untyped]

    _HAS_RICH = True
except ImportError:
    _HAS_RICH = False

if TYPE_CHECKING:
    from voidcrawl import PoolConfig

# ── Public type aliases ───────────────────────────────────────────────────

ScaleProfile = Literal["minimal", "balanced", "advanced"]
_Env = Literal["auto", "server", "pc", "embedded"]

_VALID_PROFILES: frozenset[str] = frozenset({"minimal", "balanced", "advanced"})
_VALID_ENVS: frozenset[str] = frozenset({"auto", "server", "pc", "embedded"})

# ── Exception ─────────────────────────────────────────────────────────────


class InsufficientResourcesError(RuntimeError):
    """Raised when the machine lacks the minimum resources to launch Chrome.

    Attributes:
        message: Human-readable description of the resource shortfall.
    """


# ── Profile parameter table ───────────────────────────────────────────────


@dataclass(frozen=True)
class _ProfileParams:
    ram_fraction: float  # fraction of effective RAM to allocate (0 = hard-cap only)
    per_tab_mb: int  # estimated RAM cost per open tab
    max_tabs_per_browser: int  # semaphore cap per Chrome process
    max_browsers: int  # 0 = unlimited, compute from tabs
    headless_forced: bool  # True = always headless regardless of display
    tab_max_idle_secs: int  # idle eviction timeout


_PROFILE_PARAMS: dict[str, _ProfileParams] = {
    "minimal": _ProfileParams(
        ram_fraction=0.0,
        per_tab_mb=80,
        max_tabs_per_browser=4,
        max_browsers=1,
        headless_forced=True,
        tab_max_idle_secs=20,
    ),
    "balanced": _ProfileParams(
        ram_fraction=0.40,
        per_tab_mb=100,
        max_tabs_per_browser=15,
        max_browsers=4,
        headless_forced=False,
        tab_max_idle_secs=60,
    ),
    "advanced": _ProfileParams(
        ram_fraction=0.90,
        per_tab_mb=120,
        max_tabs_per_browser=30,
        max_browsers=0,
        headless_forced=True,
        tab_max_idle_secs=120,
    ),
}

# ── Module-level resource init (read once at import) ─────────────────────

try:
    import resource as _resource_mod

    _fd_soft_limit: int = int(_resource_mod.getrlimit(_resource_mod.RLIMIT_NOFILE)[0])
except (ImportError, OSError, AttributeError):
    _fd_soft_limit = 1024

# ── Data classes ──────────────────────────────────────────────────────────


@dataclass
class ResourceSnapshot:
    """Point-in-time measurement of available system resources."""

    free_ram_mb: int
    total_ram_mb: int
    cpu_cores: int
    load_avg_1m: float
    swap_used_mb: int
    fd_soft_limit: int
    has_display: bool
    in_container: bool
    cgroup_mem_limit_mb: int | None = field(default=None)

    @property
    def effective_ram_mb(self) -> int:
        """Effective available RAM — cgroup limit when set, else free RAM."""
        if self.cgroup_mem_limit_mb is not None:
            return min(self.cgroup_mem_limit_mb, self.free_ram_mb)
        return self.free_ram_mb


@dataclass
class ScaleReport:
    """Computed pool recommendations for a profile and resource snapshot."""

    snapshot: ResourceSnapshot
    detected_env: Literal["server", "pc", "embedded"]
    profile: ScaleProfile
    browsers: int
    tabs_per_browser: int
    headless: bool
    tab_max_idle_secs: int
    warnings: list[str] = field(default_factory=list)

    @property
    def total_tabs(self) -> int:
        """Total concurrent tabs across all browsers."""
        return self.browsers * self.tabs_per_browser

    def to_pool_config(self) -> PoolConfig:
        """Convert to a :class:`~voidcrawl.PoolConfig` ready for
        :class:`~voidcrawl.BrowserPool`."""
        from voidcrawl import BrowserConfig, PoolConfig  # noqa: PLC0415

        return PoolConfig(
            browsers=self.browsers,
            tabs_per_browser=self.tabs_per_browser,
            tab_max_idle_secs=self.tab_max_idle_secs,
            browser=BrowserConfig(headless=self.headless),
        )

    def to_dict(self) -> dict[str, object]:
        """Serialise to a JSON-compatible dict (for ``--json`` scripted mode)."""
        return _report_to_dict(self)

    def print_report(self) -> None:
        """Print a human-readable summary. Uses rich markup if available."""
        _print_report(self)


# ── Private measurement helpers ───────────────────────────────────────────


def _read_proc_meminfo() -> dict[str, int]:
    """Parse ``/proc/meminfo`` into ``{key: value_kb}``."""
    result: dict[str, int] = {}
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            parts = line.split()
            if len(parts) >= 2:
                key = parts[0].rstrip(":")
                with contextlib.suppress(ValueError):
                    result[key] = int(parts[1])
    except OSError:
        pass
    return result


def _read_macos_ram_mb() -> tuple[int, int]:
    """Return ``(free_mb, total_mb)`` on macOS via ``sysctl``."""
    try:
        total_bytes = int(
            subprocess.check_output(
                ["/usr/sbin/sysctl", "-n", "hw.memsize"],
                stderr=subprocess.DEVNULL,
                timeout=2,
            ).strip()
        )
        total_mb = total_bytes // (1024 * 1024)
        # vm.page_free_count * page size is the best proxy for free RAM on macOS
        page_size_out = subprocess.check_output(
            ["/usr/sbin/sysctl", "-n", "hw.pagesize"],
            stderr=subprocess.DEVNULL,
            timeout=2,
        ).strip()
        free_pages_out = subprocess.check_output(
            ["/usr/sbin/sysctl", "-n", "vm.page_free_count"],
            stderr=subprocess.DEVNULL,
            timeout=2,
        ).strip()
        free_mb = (int(free_pages_out) * int(page_size_out)) // (1024 * 1024)
    except (subprocess.SubprocessError, ValueError, OSError):
        return 0, 0
    else:
        return free_mb, total_mb


def _read_ram_mb() -> tuple[int, int]:
    """Return ``(free_mb, total_mb)``."""
    meminfo = _read_proc_meminfo()
    if meminfo:
        free = meminfo.get("MemAvailable", meminfo.get("MemFree", 0)) // 1024
        total = meminfo.get("MemTotal", 0) // 1024
        return free, total
    if sys.platform == "darwin":
        return _read_macos_ram_mb()
    return 0, 0


def _read_swap_used_mb() -> int:
    """Return MB of swap currently in use."""
    meminfo = _read_proc_meminfo()
    if meminfo:
        total = meminfo.get("SwapTotal", 0)
        free = meminfo.get("SwapFree", 0)
        return max(0, (total - free) // 1024)
    return 0


def _read_cgroup_mem_limit_mb() -> int | None:
    """Read the Docker ``--memory`` cgroup limit in MB, or ``None`` if unlimited."""
    cgroup_v2_unlimited = "max"
    cgroup_v1_unlimited = 2**60  # ~1 EiB — kernel sentinel for no limit

    # cgroupsv2
    try:
        raw = Path("/sys/fs/cgroup/memory.max").read_text().strip()
        if raw == cgroup_v2_unlimited:
            return None
        return int(raw) // (1024 * 1024)
    except (OSError, ValueError):
        pass

    # cgroupsv1
    try:
        limit_bytes = int(
            Path("/sys/fs/cgroup/memory/memory.limit_in_bytes").read_text().strip()
        )
        if limit_bytes >= cgroup_v1_unlimited:
            return None
        return limit_bytes // (1024 * 1024)
    except (OSError, ValueError):
        pass

    return None


def _detect_container() -> bool:
    """Return ``True`` when running inside a Docker/OCI container."""
    if Path("/.dockerenv").exists():
        return True
    try:
        cgroup = Path("/proc/1/cgroup").read_text()
        return any(kw in cgroup for kw in ("docker", "containerd", "kubepods"))
    except OSError:
        return False


# ── Public API ────────────────────────────────────────────────────────────


def detect_resources() -> ResourceSnapshot:
    """Measure current system resources.

    Returns:
        A :class:`ResourceSnapshot` reflecting the present machine state.
    """
    free_mb, total_mb = _read_ram_mb()

    try:
        load_avg_1m: float = os.getloadavg()[0]
    except (OSError, AttributeError):
        load_avg_1m = 0.0

    return ResourceSnapshot(
        free_ram_mb=free_mb,
        total_ram_mb=total_mb,
        cpu_cores=os.cpu_count() or 1,
        load_avg_1m=load_avg_1m,
        swap_used_mb=_read_swap_used_mb(),
        fd_soft_limit=_fd_soft_limit,
        has_display=bool(
            os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")
        ),
        in_container=_detect_container(),
        cgroup_mem_limit_mb=_read_cgroup_mem_limit_mb(),
    )


def _detect_env(snapshot: ResourceSnapshot) -> Literal["server", "pc", "embedded"]:
    if snapshot.in_container:
        return "server"
    if snapshot.free_ram_mb < 1500:
        return "embedded"
    if snapshot.has_display:
        return "pc"
    return "server"


def _compute_tabs(
    profile: str,
    snapshot: ResourceSnapshot,
    params: _ProfileParams,
) -> tuple[int, int]:
    """Return ``(browsers, tabs_per_browser)`` for *profile* and *snapshot*."""
    if profile == "minimal":
        return 1, 2

    ram = snapshot.effective_ram_mb
    ram_ceiling = int(ram * params.ram_fraction / params.per_tab_mb) if ram > 0 else 4
    cpu_ceiling = snapshot.cpu_cores * 10
    fd_ceiling = max(1, (snapshot.fd_soft_limit - 500) // 50)
    max_tabs = max(4, min(ram_ceiling, cpu_ceiling, fd_ceiling))

    if profile == "balanced":
        max_tabs = min(max_tabs, 60)
        tabs_per = min(params.max_tabs_per_browser, max_tabs)
        browsers = max(1, min(max_tabs // max(1, tabs_per), params.max_browsers))
    else:  # advanced
        tabs_per = params.max_tabs_per_browser
        browsers = max(1, math.ceil(max_tabs / tabs_per))

    return browsers, tabs_per


def compute_scale(
    profile: ScaleProfile = "balanced",
    *,
    env: _Env = "auto",
    snapshot: ResourceSnapshot | None = None,
) -> ScaleReport:
    """Measure resources and compute a safe pool configuration.

    Args:
        profile: Aggressiveness of resource use. One of ``"minimal"``,
            ``"balanced"`` (default), or ``"advanced"``.
        env: Environment hint. ``"auto"`` detects automatically from
            system facts (display server, container, RAM).
        snapshot: Pre-measured :class:`ResourceSnapshot`. When ``None``,
            :func:`detect_resources` is called automatically.

    Returns:
        A :class:`ScaleReport` with ``browsers``, ``tabs_per_browser``, and
        :meth:`~ScaleReport.to_pool_config` ready to use.

    Raises:
        ValueError: When *profile* or *env* is not a recognised value.
        InsufficientResourcesError: When the machine lacks the minimum
            resources required to launch Chrome.

    Example:
        >>> report = compute_scale("balanced")
        >>> pool_cfg = report.to_pool_config()
        >>> print(report.browsers, report.tabs_per_browser)
    """
    if profile not in _VALID_PROFILES:
        raise ValueError(
            f"Unknown profile {profile!r}. Choose from: {sorted(_VALID_PROFILES)}"
        )
    if env not in _VALID_ENVS:
        raise ValueError(f"Unknown env {env!r}. Choose from: {sorted(_VALID_ENVS)}")

    snap = snapshot if snapshot is not None else detect_resources()

    # Hard gates — refuse before any Chrome is launched
    effective_ram = snap.effective_ram_mb
    if effective_ram < 600:
        raise InsufficientResourcesError(
            f"Insufficient RAM: {effective_ram} MB available. "
            "Chrome requires at least 600 MB."
        )
    if snap.fd_soft_limit < 256:
        raise InsufficientResourcesError(
            f"File-descriptor limit too low ({snap.fd_soft_limit}). "
            "Run `ulimit -n 65536` or set /etc/security/limits.conf."
        )

    warnings: list[str] = []
    resolved_profile: ScaleProfile = profile

    # Swap guard — advanced on a swapping machine is dangerous
    if snap.swap_used_mb > 0 and profile == "advanced":
        warnings.append(
            f"Swap is active ({snap.swap_used_mb} MB used) — "
            "downgrading from 'advanced' to 'balanced' to protect stability."
        )
        resolved_profile = "balanced"

    # High CPU load warning
    load_ratio = snap.load_avg_1m / max(1, snap.cpu_cores)
    if load_ratio > 1.5:
        warnings.append(
            f"High CPU load ({snap.load_avg_1m:.1f} / {snap.cpu_cores} cores = "
            f"{load_ratio:.0%}). Tab count may be halved by Chrome scheduler."
        )

    detected_env = (
        _detect_env(snap) if env == "auto" else env  # type: ignore[assignment]
    )

    params = _PROFILE_PARAMS[resolved_profile]
    browsers, tabs_per = _compute_tabs(resolved_profile, snap, params)

    headless = params.headless_forced or not snap.has_display

    return ScaleReport(
        snapshot=snap,
        detected_env=detected_env,
        profile=resolved_profile,
        browsers=browsers,
        tabs_per_browser=tabs_per,
        headless=headless,
        tab_max_idle_secs=params.tab_max_idle_secs,
        warnings=warnings,
    )


def _default_base_port() -> int:
    """Resolve the default CDP base port from env, falling back to 9222.

    Honors the ``CDP_PORT_BASE`` env var so blocked / taken ports can be
    worked around without editing config. Invalid values silently fall
    back to 9222 — an unparseable override shouldn't brick the container.
    """
    raw = os.environ.get("CDP_PORT_BASE")
    if raw is None:
        return 9222
    try:
        return int(raw)
    except ValueError:
        return 9222


def generate_supervisord_conf(report: ScaleReport, base_port: int | None = None) -> str:
    """Build a supervisord.conf string launching *report.browsers* Chrome instances.

    Args:
        report: A :class:`ScaleReport` from :func:`compute_scale`.
        base_port: First CDP debugging port. Subsequent browsers use
            ``base_port + 1``, ``base_port + 2``, etc. When ``None``
            (default), reads the ``CDP_PORT_BASE`` env var and falls back
            to ``9222``.

    Returns:
        A complete supervisord.conf file as a string, ready to write to disk.

    Example:
        >>> conf = generate_supervisord_conf(report)
        >>> Path("/tmp/supervisord.conf").write_text(conf)
    """
    if base_port is None:
        base_port = _default_base_port()
    chrome = "/usr/bin/chromium"
    # Per-Chrome profile dir under CHROME_PROFILES_DIR (default /tmp, ephemeral;
    # set to a mounted volume like /profiles to persist logins/cookies/clearance).
    profiles_dir = os.environ.get("CHROME_PROFILES_DIR", "/tmp").rstrip("/") or "/tmp"
    # Hardware GPU via ANGLE/Vulkan (NOT --disable-gpu, which forces SwiftShader
    # software WebGL — a strong bot signal). Requires Mesa drivers in the image
    # + /dev/dri passthrough (see docker/Dockerfile and docker-compose.yml).
    # Mirrors the low-noise core + GPU group of DEFAULT_CHROME_ARGS in
    # crates/core/src/session.rs; keep the two in sync.
    base_flags = (
        "--no-sandbox"
        " --remote-allow-origins=*"
        " --enable-gpu"
        " --ignore-gpu-blocklist"
        " --use-angle=vulkan"
        " --disable-gpu-sandbox"
        " --disable-dev-shm-usage"
        " --disable-infobars"
        " --disable-breakpad"
        " --disable-session-crashed-bubble"
        " --disable-search-engine-choice-screen"
        " --no-first-run"
        " --no-service-autorun"
        " --no-default-browser-check"
        " --no-pings"
        " --password-store=basic"
        " --homepage=about:blank"
    )
    headless_flag = "--headless=new " if report.headless else ""
    headless_automation_flag = (
        " --disable-blink-features=AutomationControlled" if report.headless else ""
    )

    sections = [
        "[supervisord]",
        "nodaemon=true",
        "logfile=/var/log/supervisord.log",
        "pidfile=/var/run/supervisord.pid",
        "",
    ]

    for i in range(report.browsers):
        port = base_port + i
        name = f"chrome-debug-{i + 1}"
        cmd = (
            f"{chrome} {headless_flag}{base_flags}{headless_automation_flag}"
            f" --remote-debugging-port={port}"
            f" --user-data-dir={profiles_dir}/chrome-profile-{i + 1}"
        )
        sections += [
            f"[program:{name}]",
            f"command={cmd}",
            "autostart=true",
            "autorestart=true",
            f"stdout_logfile=/var/log/{name}.log",
            f"stderr_logfile=/var/log/{name}-err.log",
            "priority=10",
            "",
        ]

    return "\n".join(sections)


# ── Report serialisation / printing ──────────────────────────────────────


def _report_to_dict(report: ScaleReport) -> dict[str, object]:
    s = report.snapshot
    base_port = _default_base_port()
    ws_urls = ",".join(
        f"http://localhost:{base_port + i}" for i in range(report.browsers)
    )
    return {
        "detected_env": report.detected_env,
        "profile": report.profile,
        "browsers": report.browsers,
        "tabs_per_browser": report.tabs_per_browser,
        "total_tabs": report.total_tabs,
        "headless": report.headless,
        "tab_max_idle_secs": report.tab_max_idle_secs,
        "warnings": report.warnings,
        "snapshot": {
            "free_ram_mb": s.free_ram_mb,
            "total_ram_mb": s.total_ram_mb,
            "effective_ram_mb": s.effective_ram_mb,
            "cpu_cores": s.cpu_cores,
            "load_avg_1m": s.load_avg_1m,
            "swap_used_mb": s.swap_used_mb,
            "fd_soft_limit": s.fd_soft_limit,
            "has_display": s.has_display,
            "in_container": s.in_container,
            "cgroup_mem_limit_mb": s.cgroup_mem_limit_mb,
        },
        "env_vars": {
            "SCALE_PROFILE": report.profile,
            "BROWSER_COUNT": str(report.browsers),
            "TABS_PER_BROWSER": str(report.tabs_per_browser),
            "CHROME_HEADLESS": "0" if not report.headless else "1",
            "TAB_MAX_IDLE_SECS": str(report.tab_max_idle_secs),
            "CHROME_WS_URLS": ws_urls,
        },
    }


def _print_report(report: ScaleReport) -> None:
    s = report.snapshot
    ws_urls = ",".join(f"http://localhost:{9222 + i}" for i in range(report.browsers))
    lines = [
        "VoidCrawl Scale Report",
        "=" * 40,
        f"  Environment   : {report.detected_env}",
        f"  Profile       : {report.profile}",
        "",
        "  System",
        f"    RAM free    : {s.free_ram_mb:,} MB  (total {s.total_ram_mb:,} MB)",
        f"    CPU cores   : {s.cpu_cores}",
        f"    Load avg    : {s.load_avg_1m:.2f}",
        f"    Swap active : {f'yes ({s.swap_used_mb} MB)' if s.swap_used_mb else 'no'}",
        f"    Container   : {'yes' if s.in_container else 'no'}",
        f"    Display     : {'yes' if s.has_display else 'no'}",
    ]
    if s.cgroup_mem_limit_mb is not None:
        lines.append(f"    cgroup limit: {s.cgroup_mem_limit_mb:,} MB")

    lines += [
        "",
        "  Recommended Pool",
        f"    browsers        : {report.browsers}",
        f"    tabs/browser    : {report.tabs_per_browser}",
        f"    total tabs      : {report.total_tabs}",
        f"    headless        : {report.headless}",
        f"    idle evict secs : {report.tab_max_idle_secs}",
        "",
        "  Pool env vars",
        f"    SCALE_PROFILE={report.profile}",
        f"    BROWSER_COUNT={report.browsers}",
        f"    TABS_PER_BROWSER={report.tabs_per_browser}",
        f"    CHROME_HEADLESS={'1' if report.headless else '0'}",
        f"    TAB_MAX_IDLE_SECS={report.tab_max_idle_secs}",
        f"    CHROME_WS_URLS={ws_urls}",
    ]

    if report.warnings:
        lines += ["", "  Warnings"]
        lines.extend(f"    * {w}" for w in report.warnings)

    text = "\n".join(lines)
    if not _HAS_RICH:
        print(text)
        return
    rprint(f"[bold cyan]{lines[0]}[/bold cyan]")
    for line in lines[1:]:
        if line.startswith("  ") and ":" in line:
            key, _, val = line.partition(":")
            rprint(f"[dim]{key}:[/dim][green]{val}[/green]")
        elif line.startswith("    *"):
            rprint(f"[yellow]{line}[/yellow]")
        else:
            rprint(line)


# ── __main__ ─────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import argparse
    import json as _json

    _parser = argparse.ArgumentParser(
        prog="python -m voidcrawl.scale",
        description="Detect system resources and recommend a VoidCrawl pool config.",
    )
    _parser.add_argument(
        "--profile",
        choices=["minimal", "balanced", "advanced"],
        default=None,
        help="Scale profile (default: $SCALE_PROFILE env var, then 'balanced')",
    )
    _parser.add_argument(
        "--env",
        choices=["auto", "server", "pc", "embedded"],
        default="auto",
        help="Environment hint (default: auto-detect)",
    )
    _parser.add_argument(
        "--json",
        action="store_true",
        dest="as_json",
        help="Output machine-readable JSON",
    )
    _args = _parser.parse_args()

    _profile: ScaleProfile = _args.profile or os.environ.get(
        "SCALE_PROFILE", "balanced"
    )  # type: ignore[assignment]

    try:
        _report = compute_scale(profile=_profile, env=_args.env)
        if _args.as_json:
            print(_json.dumps(_report.to_dict(), indent=2))
        else:
            _report.print_report()
    except InsufficientResourcesError as _exc:
        print(f"ERROR: {_exc}", file=sys.stderr)
        sys.exit(1)
    except ValueError as _exc:
        print(f"ERROR: {_exc}", file=sys.stderr)
        sys.exit(2)
