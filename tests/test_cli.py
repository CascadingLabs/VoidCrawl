"""Unit tests for voidcrawl.cli — Click commands with CliRunner."""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from click.testing import CliRunner

from voidcrawl.cli import (
    _detect_gpu,
    _find_docker_dir,
    main,
)
from voidcrawl.scale import (
    InsufficientResourcesError,
    ResourceSnapshot,
    ScaleReport,
)


@pytest.fixture
def runner() -> CliRunner:
    return CliRunner()


# ── _find_docker_dir ─────────────────────────────────────────────────────


class TestFindDockerDir:
    def test_finds_from_cwd(self, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()
        with patch("voidcrawl.cli.Path") as mock_path:
            mock_cwd = MagicMock()
            mock_cwd.__truediv__ = MagicMock(return_value=docker_dir)
            mock_path.cwd.return_value = mock_cwd
            mock_path.return_value.parent.parent.__truediv__ = MagicMock(
                return_value=Path("/nonexistent")
            )
            result = _find_docker_dir()
            assert result == docker_dir

    def test_raises_when_not_found(self, tmp_path: Path) -> None:
        with patch("voidcrawl.cli.Path") as mock_path:
            nonexistent = tmp_path / "nope"
            mock_cwd = MagicMock()
            mock_cwd.__truediv__ = MagicMock(return_value=nonexistent)
            mock_path.cwd.return_value = mock_cwd

            file_mock = MagicMock()
            file_mock.parent.parent.__truediv__ = MagicMock(
                return_value=tmp_path / "also_nope"
            )
            mock_path.return_value = file_mock

            with pytest.raises(Exception, match="Cannot find docker"):
                _find_docker_dir()


# ── _detect_gpu ──────────────────────────────────────────────────────────


class TestDetectGpu:
    def test_amd(self, tmp_path: Path) -> None:
        with patch("voidcrawl.cli.Path") as mock_path:
            mock_dri = MagicMock()
            mock_dri.exists.return_value = True

            mock_driver = MagicMock()
            mock_driver.resolve.return_value = MagicMock(name="amdgpu")
            mock_driver.resolve.return_value.name = "amdgpu"

            mock_nvidia = MagicMock()
            mock_nvidia.exists.return_value = False

            def path_factory(p: str) -> MagicMock:
                if p == "/dev/dri/renderD128":
                    return mock_dri
                if p == "/sys/class/drm/renderD128/device/driver":
                    return mock_driver
                if p == "/dev/nvidia0":
                    return mock_nvidia
                return MagicMock()

            mock_path.side_effect = path_factory
            assert _detect_gpu() == "amd"

    def test_falls_back_to_cpu(self) -> None:
        with patch("voidcrawl.cli.Path") as mock_path:
            mock_obj = MagicMock()
            mock_obj.exists.return_value = False
            mock_path.return_value = mock_obj
            mock_path.side_effect = lambda p: mock_obj
            assert _detect_gpu() == "cpu"


# ── vc scale ─────────────────────────────────────────────────────────────

_FAKE_SNAPSHOT = ResourceSnapshot(
    free_ram_mb=8000,
    total_ram_mb=16000,
    cpu_cores=4,
    load_avg_1m=1.0,
    swap_used_mb=0,
    fd_soft_limit=65536,
    has_display=False,
    in_container=False,
)


def _fake_report(**overrides: object) -> ScaleReport:
    defaults = {
        "snapshot": _FAKE_SNAPSHOT,
        "detected_env": "server",
        "profile": "balanced",
        "browsers": 2,
        "tabs_per_browser": 15,
        "headless": True,
        "tab_max_idle_secs": 60,
    }
    defaults.update(overrides)
    return ScaleReport(**defaults)


class TestScaleCommand:
    def test_scale_default(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.return_value = _fake_report()
            result = runner.invoke(main, ["scale"])
            assert result.exit_code == 0
            assert "VoidCrawl Scale Report" in result.output

    def test_scale_json(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.return_value = _fake_report()
            result = runner.invoke(main, ["scale", "--json"])
            assert result.exit_code == 0
            data = json.loads(result.output)
            assert data["browsers"] == 2
            assert data["profile"] == "balanced"

    def test_scale_with_profile(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.return_value = _fake_report(
                profile="minimal",
                browsers=1,
                tabs_per_browser=2,
                tab_max_idle_secs=20,
            )
            result = runner.invoke(main, ["scale", "--profile", "minimal"])
            assert result.exit_code == 0
            mock_cs.assert_called_once_with(profile="minimal", env="auto")

    def test_scale_with_env_hint(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.return_value = _fake_report(
                detected_env="embedded",
                browsers=1,
                tabs_per_browser=4,
            )
            result = runner.invoke(main, ["scale", "--env", "embedded"])
            assert result.exit_code == 0
            mock_cs.assert_called_once_with(profile="balanced", env="embedded")

    def test_scale_insufficient_resources(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.side_effect = InsufficientResourcesError("Not enough RAM")
            result = runner.invoke(main, ["scale"])
            assert result.exit_code != 0
            assert "Not enough RAM" in result.output

    def test_scale_invalid_profile_from_env(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.side_effect = ValueError("Unknown profile 'turbo'")
            result = runner.invoke(main, ["scale"], env={"SCALE_PROFILE": "turbo"})
            assert result.exit_code != 0

    def test_scale_reads_env_var(self, runner: CliRunner) -> None:
        with patch("voidcrawl.cli.compute_scale") as mock_cs:
            mock_cs.return_value = _fake_report(
                profile="advanced",
                browsers=4,
                tabs_per_browser=30,
                tab_max_idle_secs=120,
            )
            result = runner.invoke(main, ["scale"], env={"SCALE_PROFILE": "advanced"})
            assert result.exit_code == 0
            mock_cs.assert_called_once_with(profile="advanced", env="auto")


# ── vc docker up ─────────────────────────────────────────────────────────


class TestDockerUp:
    def test_headless_up(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()
        (docker_dir / "docker-compose.yml").touch()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "up"])
            assert result.exit_code == 0
            mock_compose.assert_called_once()
            cmd = mock_compose.call_args[0][0]
            assert "up" in cmd
            assert str(docker_dir / "docker-compose.yml") in cmd

    def test_headless_up_detach(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "up", "-d"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "-d" in cmd

    def test_headless_up_build(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "up", "--build"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "--build" in cmd

    def test_headful_up(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._detect_gpu", return_value="amd"),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "up", "--headful"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "docker-compose.headful.yml" in str(cmd)
            assert "--profile" in cmd
            assert "amd" in cmd

    def test_headful_up_explicit_gpu(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(
                main,
                ["docker", "up", "--headful", "--gpu", "nvidia"],
            )
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "nvidia" in cmd

    def test_headful_invalid_resolution(
        self, runner: CliRunner, tmp_path: Path
    ) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with patch(
            "voidcrawl.cli._find_docker_dir",
            return_value=docker_dir,
        ):
            result = runner.invoke(
                main,
                ["docker", "up", "--headful", "--res", "not_valid"],
            )
            assert result.exit_code != 0

    def test_headful_custom_resolution(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._detect_gpu", return_value="cpu"),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(
                main,
                ["docker", "up", "--headful", "--res", "2560x1440"],
            )
            assert result.exit_code == 0
            env = mock_compose.call_args[1].get("env") or mock_compose.call_args[0][1]
            assert env["VNC_WIDTH"] == "2560"
            assert env["VNC_HEIGHT"] == "1440"


# ── vc docker down ───────────────────────────────────────────────────────


class TestDockerDown:
    def test_headless_down(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "down"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "down" in cmd

    def test_headful_down(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._detect_gpu", return_value="intel"),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "down", "--headful"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "docker-compose.headful.yml" in str(cmd)
            assert "intel" in cmd


# ── vc docker logs ───────────────────────────────────────────────────────


class TestDockerLogs:
    def test_headless_logs(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "logs"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "logs" in cmd

    def test_headless_logs_follow(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "logs", "-f"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "-f" in cmd

    def test_headful_logs(self, runner: CliRunner, tmp_path: Path) -> None:
        docker_dir = tmp_path / "docker"
        docker_dir.mkdir()

        with (
            patch(
                "voidcrawl.cli._find_docker_dir",
                return_value=docker_dir,
            ),
            patch("voidcrawl.cli._detect_gpu", return_value="amd"),
            patch("voidcrawl.cli._compose") as mock_compose,
        ):
            result = runner.invoke(main, ["docker", "logs", "--headful"])
            assert result.exit_code == 0
            cmd = mock_compose.call_args[0][0]
            assert "docker-compose.headful.yml" in str(cmd)


# ── vc --help ────────────────────────────────────────────────────────────


class TestHelp:
    def test_root_help(self, runner: CliRunner) -> None:
        result = runner.invoke(main, ["--help"])
        assert result.exit_code == 0
        assert "VoidCrawl" in result.output

    def test_docker_help(self, runner: CliRunner) -> None:
        result = runner.invoke(main, ["docker", "--help"])
        assert result.exit_code == 0
        assert "Manage" in result.output

    def test_scale_help(self, runner: CliRunner) -> None:
        result = runner.invoke(main, ["scale", "--help"])
        assert result.exit_code == 0
        assert "profile" in result.output.lower()
