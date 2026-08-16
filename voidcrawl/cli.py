"""CLI entry point for VoidCrawl — ``vc`` / ``voidcrawl`` commands."""

from __future__ import annotations

import json as _json
import os
import subprocess
import sys
from pathlib import Path

import click

from voidcrawl.scale import InsufficientResourcesError, compute_scale

# ── Helpers ──────────────────────────────────────────────────────────────


def _find_docker_dir() -> Path:
    """Return the docker/ directory, searching cwd then package-relative."""
    for candidate in (
        Path.cwd() / "docker",
        Path(__file__).parent.parent / "docker",
    ):
        if candidate.is_dir():
            return candidate
    raise click.ClickException(
        "Cannot find docker/ directory. "
        "Run this command from the VoidCrawl project root."
    )


def _detect_gpu() -> str:
    """Return the GPU profile string by reading the kernel DRM driver."""
    if Path("/dev/dri/renderD128").exists():
        try:
            driver = Path("/sys/class/drm/renderD128/device/driver").resolve().name
        except OSError:
            driver = "unknown"
        if driver == "amdgpu":
            return "amd"
        if driver in ("i915", "xe"):
            return "intel"
        if driver == "nvidia":
            return "nvidia"
    if Path("/dev/nvidia0").exists():
        return "nvidia"
    return "cpu"


def _compose(cmd: list[str], env: dict[str, str] | None = None) -> None:
    """Run a docker compose command and forward its exit code."""
    try:
        result = subprocess.run(cmd, env=env, check=False)
    except FileNotFoundError:
        raise click.ClickException(
            "docker not found — is Docker installed and in PATH?"
        ) from None
    sys.exit(result.returncode)


# ── Root group ───────────────────────────────────────────────────────────


@click.group()
def main() -> None:
    """VoidCrawl — Rust-native CDP browser automation."""


# ── docker subgroup ──────────────────────────────────────────────────────


@main.group()
def docker() -> None:
    """Manage VoidCrawl Docker containers."""


@docker.command("up")
@click.option(
    "--headful",
    is_flag=True,
    help="Start headful Chrome with Sway + VNC.",
)
@click.option(
    "--gpu",
    type=click.Choice(["amd", "nvidia", "intel", "cpu"]),
    default=None,
    help="GPU profile (headful only). Auto-detected when omitted.",
)
@click.option(
    "--res",
    default="1920x1080",
    metavar="WxH",
    help="Headful output resolution.  [default: 1920x1080]",
)
@click.option("-d", "--detach", is_flag=True, help="Run in the background.")
@click.option("--build", is_flag=True, help="Rebuild the Docker image first.")
def docker_up(
    headful: bool,
    gpu: str | None,
    res: str,
    detach: bool,
    build: bool,
) -> None:
    """Start VoidCrawl Chrome container(s).

    \b
    Headless (default):
      CDP  localhost:9222, localhost:9223

    \b
    Headful:
      CDP     localhost:19222, localhost:19223
      Viewer  disabled by default; enable VIEWER_MODE=local, then run
              docker/viewer.sh open --browser <n> --ttl <duration>
    """
    docker_dir = _find_docker_dir()

    if headful:
        profile = gpu or _detect_gpu()
        detected = " (auto-detected)" if gpu is None else ""

        try:
            width, height = res.split("x", 1)
            int(width)
            int(height)
        except ValueError:
            raise click.BadParameter(
                f"expected WxH (e.g. 1920x1080), got {res!r}",
                param_hint="--res",
            ) from None

        click.echo("Starting headful Chrome container...")
        click.echo(f"  GPU profile : {profile}{detected}")
        click.echo(f"  Resolution  : {res}")
        click.echo("  Viewer      : disabled (set VIEWER_MODE=local to enable)")
        click.echo(
            "                then use docker/viewer.sh open --browser <n> --ttl 15m"
        )
        click.echo("  CDP         : localhost:19222, localhost:19223")
        click.echo("")

        cmd = [
            "docker",
            "compose",
            "-f",
            str(docker_dir / "docker-compose.headful.yml"),
            "--profile",
            profile,
            "up",
        ]
        if build:
            cmd.append("--build")
        if detach:
            cmd.append("-d")

        _compose(cmd, env={**os.environ, "VNC_WIDTH": width, "VNC_HEIGHT": height})
    else:
        click.echo("Starting headless Chrome container...")
        click.echo("  CDP : localhost:9222, localhost:9223")
        click.echo("")

        cmd = [
            "docker",
            "compose",
            "-f",
            str(docker_dir / "docker-compose.yml"),
            "up",
        ]
        if build:
            cmd.append("--build")
        if detach:
            cmd.append("-d")

        _compose(cmd)


@docker.command("down")
@click.option("--headful", is_flag=True, help="Stop the headful container.")
@click.option(
    "--gpu",
    type=click.Choice(["amd", "nvidia", "intel", "cpu"]),
    default=None,
    help="GPU profile (headful only). Auto-detected when omitted.",
)
def docker_down(headful: bool, gpu: str | None) -> None:
    """Stop VoidCrawl Chrome container(s)."""
    docker_dir = _find_docker_dir()

    if headful:
        profile = gpu or _detect_gpu()
        cmd = [
            "docker",
            "compose",
            "-f",
            str(docker_dir / "docker-compose.headful.yml"),
            "--profile",
            profile,
            "down",
        ]
    else:
        cmd = [
            "docker",
            "compose",
            "-f",
            str(docker_dir / "docker-compose.yml"),
            "down",
        ]

    _compose(cmd)


@main.command("scale")
@click.option(
    "--profile",
    type=click.Choice(["minimal", "balanced", "advanced"]),
    default=None,
    help="Scale profile (default: $SCALE_PROFILE env var, then 'balanced').",
)
@click.option(
    "--env",
    type=click.Choice(["auto", "server", "pc", "embedded"]),
    default="auto",
    show_default=True,
    help="Environment hint for resource detection.",
)
@click.option("--json", "as_json", is_flag=True, help="Output machine-readable JSON.")
def scale_cmd(profile: str | None, env: str, as_json: bool) -> None:
    """Detect system resources and recommend a pool configuration.

    \b
    Profiles:
      minimal   — embedded / CI  (1 browser, ≤4 tabs)
      balanced  — developer PC   (40 % RAM, ≤60 tabs)
      advanced  — dedicated server (90 % RAM, p95 capacity)

    \b
    Examples:
      vc scale
      vc scale --profile advanced
      vc scale --json
      SCALE_PROFILE=advanced vc scale
    """
    resolved = profile or os.environ.get("SCALE_PROFILE", "balanced")
    try:
        report = compute_scale(profile=resolved, env=env)  # type: ignore[arg-type]
    except InsufficientResourcesError as exc:
        raise click.ClickException(str(exc)) from exc
    except ValueError as exc:
        raise click.UsageError(str(exc)) from exc

    if as_json:
        click.echo(_json.dumps(report.to_dict(), indent=2))
    else:
        report.print_report()


@docker.command("logs")
@click.option("--headful", is_flag=True, help="Show logs from the headful container.")
@click.option("-f", "--follow", is_flag=True, help="Follow log output.")
@click.option(
    "--gpu",
    type=click.Choice(["amd", "nvidia", "intel", "cpu"]),
    default=None,
    help="GPU profile (headful only). Auto-detected when omitted.",
)
def docker_logs(headful: bool, follow: bool, gpu: str | None) -> None:
    """Tail VoidCrawl container logs."""
    docker_dir = _find_docker_dir()

    if headful:
        profile = gpu or _detect_gpu()
        cmd = [
            "docker",
            "compose",
            "-f",
            str(docker_dir / "docker-compose.headful.yml"),
            "--profile",
            profile,
            "logs",
        ]
    else:
        cmd = [
            "docker",
            "compose",
            "-f",
            str(docker_dir / "docker-compose.yml"),
            "logs",
        ]

    if follow:
        cmd.append("-f")

    _compose(cmd)
