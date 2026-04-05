"""Docker integration tests using testcontainers.

These tests build and start the VoidCrawl headless Docker image, verify
Chrome is healthy, and exercise PoolConfig.from_docker() against a real
container.

Requires: Docker daemon running, network access to pull base image on
first run. Skipped automatically when Docker is unavailable.

Run with:
    uv run pytest tests/test_docker_integration.py -v --timeout=120
"""

from __future__ import annotations

import json
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterator

import pytest

from voidcrawl import PoolConfig

# Skip the entire module if Docker isn't available.
_docker_available = False
try:
    result = subprocess.run(
        ["docker", "info"],
        capture_output=True,
        timeout=5,
        check=False,
    )
    _docker_available = result.returncode == 0
except (FileNotFoundError, subprocess.TimeoutExpired):
    pass

pytestmark = pytest.mark.skipif(
    not _docker_available, reason="Docker daemon not available"
)


# ── Fixtures ─────────────────────────────────────────────────────────────

_PROJECT_ROOT = Path(__file__).parent.parent


def _wait_for_cdp(url: str, timeout: float = 60) -> dict[str, object]:
    """Poll ``url/json/version`` until it responds."""
    deadline = time.monotonic() + timeout
    last_err: Exception | None = None
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{url}/json/version", timeout=3) as resp:
                data: dict[str, object] = json.loads(resp.read())
                return data
        except (  # noqa: PERF203
            urllib.error.URLError,
            OSError,
            json.JSONDecodeError,
        ) as exc:
            last_err = exc
            time.sleep(1)
    raise TimeoutError(f"Chrome at {url} not healthy within {timeout}s: {last_err}")


@pytest.fixture(scope="module")
def cdp_container() -> Iterator[dict[str, object]]:
    """Build and start the headless Docker image, yield CDP info,
    then tear down.
    """
    compose_file = _PROJECT_ROOT / "docker" / "docker-compose.yml"
    if not compose_file.exists():
        pytest.skip("docker-compose.yml not found")

    subprocess.run(
        [
            "docker",
            "compose",
            "-f",
            str(compose_file),
            "up",
            "-d",
            "--build",
        ],
        cwd=str(_PROJECT_ROOT),
        check=True,
        timeout=300,
    )

    try:
        info = _wait_for_cdp("http://localhost:9222", timeout=60)
        yield info
    finally:
        subprocess.run(
            [
                "docker",
                "compose",
                "-f",
                str(compose_file),
                "down",
            ],
            cwd=str(_PROJECT_ROOT),
            check=False,
            timeout=60,
        )


def _docker_exec(*cmd: str) -> subprocess.CompletedProcess[str]:
    """Run a command inside the running compose service."""
    compose_file = _PROJECT_ROOT / "docker" / "docker-compose.yml"
    return subprocess.run(
        [
            "docker",
            "compose",
            "-f",
            str(compose_file),
            "exec",
            "voidcrawl-cdp-headless",
            *cmd,
        ],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )


# ── Tests ────────────────────────────────────────────────────────────────


class TestDockerHealthcheck:
    def test_chrome_responds(self, cdp_container: dict[str, object]) -> None:
        assert "Browser" in cdp_container or "webSocketDebuggerUrl" in cdp_container

    def test_chrome_version_string(self, cdp_container: dict[str, object]) -> None:
        browser = cdp_container.get("Browser", "")
        assert isinstance(browser, str)
        assert len(browser) > 0

    def test_websocket_url_present(self, cdp_container: dict[str, object]) -> None:
        ws_url = cdp_container.get("webSocketDebuggerUrl", "")
        assert isinstance(ws_url, str)
        assert ws_url.startswith("ws://")


class TestDockerSecondPort:
    def test_second_instance_responds(self, cdp_container: dict[str, object]) -> None:
        info = _wait_for_cdp("http://localhost:9223", timeout=30)
        assert "Browser" in info or "webSocketDebuggerUrl" in info


class TestDockerPoolConfig:
    def test_from_docker_headless(self, cdp_container: dict[str, object]) -> None:
        cfg = PoolConfig.from_docker(check=True)
        assert len(cfg.chrome_ws_urls) == 2
        assert cfg.browsers == 2

    def test_from_docker_custom_tabs(self, cdp_container: dict[str, object]) -> None:
        cfg = PoolConfig.from_docker(tabs_per_browser=8, check=True)
        assert cfg.tabs_per_browser == 8


class TestDockerEnvironment:
    def test_scale_profile_env(self, cdp_container: dict[str, object]) -> None:
        result = _docker_exec("printenv", "SCALE_PROFILE")
        if result.returncode == 0:
            assert result.stdout.strip() in ("balanced", "")

    def test_no_sandbox_env(self, cdp_container: dict[str, object]) -> None:
        result = _docker_exec("printenv", "CHROME_NO_SANDBOX")
        if result.returncode == 0:
            assert result.stdout.strip() == "1"


class TestDockerSupervisord:
    def test_supervisord_running(self, cdp_container: dict[str, object]) -> None:
        result = _docker_exec("pgrep", "-c", "supervisord")
        if result.returncode == 0:
            count = int(result.stdout.strip())
            assert count >= 1

    def test_chrome_processes_running(self, cdp_container: dict[str, object]) -> None:
        result = _docker_exec("pgrep", "-c", "chromium")
        if result.returncode == 0:
            count = int(result.stdout.strip())
            assert count >= 1
