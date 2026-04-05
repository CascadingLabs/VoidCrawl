"""Type stubs for voidcrawl.scale."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from voidcrawl import PoolConfig

ScaleProfile = Literal["minimal", "balanced", "advanced"]
_Env = Literal["auto", "server", "pc", "embedded"]

class InsufficientResourcesError(RuntimeError): ...

@dataclass(frozen=True)
class _ProfileParams:
    ram_fraction: float
    per_tab_mb: int
    max_tabs_per_browser: int
    max_browsers: int
    headless_forced: bool
    tab_max_idle_secs: int

_PROFILE_PARAMS: dict[str, _ProfileParams]

class ResourceSnapshot:
    free_ram_mb: int
    total_ram_mb: int
    cpu_cores: int
    load_avg_1m: float
    swap_used_mb: int
    fd_soft_limit: int
    has_display: bool
    in_container: bool
    cgroup_mem_limit_mb: int | None

    def __init__(
        self,
        *,
        free_ram_mb: int,
        total_ram_mb: int,
        cpu_cores: int,
        load_avg_1m: float,
        swap_used_mb: int,
        fd_soft_limit: int,
        has_display: bool,
        in_container: bool,
        cgroup_mem_limit_mb: int | None = None,
    ) -> None: ...
    @property
    def effective_ram_mb(self) -> int: ...

class ScaleReport:
    snapshot: ResourceSnapshot
    detected_env: Literal["server", "pc", "embedded"]
    profile: ScaleProfile
    browsers: int
    tabs_per_browser: int
    headless: bool
    tab_max_idle_secs: int
    warnings: list[str]

    @property
    def total_tabs(self) -> int: ...
    def to_pool_config(self) -> PoolConfig: ...
    def to_dict(self) -> dict[str, object]: ...
    def print_report(self) -> None: ...

def detect_resources() -> ResourceSnapshot: ...
def compute_scale(
    profile: ScaleProfile = "balanced",
    *,
    env: _Env = "auto",
    snapshot: ResourceSnapshot | None = None,
) -> ScaleReport: ...
def generate_supervisord_conf(report: ScaleReport, base_port: int = 9222) -> str: ...
def _detect_env(snapshot: ResourceSnapshot) -> Literal["server", "pc", "embedded"]: ...
def _compute_tabs(
    profile: str,
    snapshot: ResourceSnapshot,
    params: _ProfileParams,
) -> tuple[int, int]: ...
