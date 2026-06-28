"""Unit tests for voidcrawl.scale — resource detection, profile computation,
supervisord generation, and serialisation."""

from __future__ import annotations

import json

import pytest

from voidcrawl.scale import (
    _PROFILE_PARAMS,
    InsufficientResourcesError,
    ResourceSnapshot,
    _compute_tabs,
    _detect_env,
    compute_scale,
    generate_supervisord_conf,
)

# ── ResourceSnapshot ─────────────────────────────────────────────────────


class TestResourceSnapshot:
    def test_effective_ram_without_cgroup(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        assert snapshot_server.cgroup_mem_limit_mb is None
        assert snapshot_server.effective_ram_mb == snapshot_server.free_ram_mb

    def test_effective_ram_with_cgroup_below_free(
        self, snapshot_container_cgroup: ResourceSnapshot
    ) -> None:
        snap = snapshot_container_cgroup
        assert snap.cgroup_mem_limit_mb is not None
        assert snap.effective_ram_mb == min(snap.cgroup_mem_limit_mb, snap.free_ram_mb)

    def test_effective_ram_cgroup_above_free(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=1000,
            total_ram_mb=8000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=True,
            cgroup_mem_limit_mb=4096,
        )
        # free < cgroup, so free wins
        assert snap.effective_ram_mb == 1000


# ── Environment detection ────────────────────────────────────────────────


class TestDetectEnv:
    def test_container_is_server(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=8000,
            total_ram_mb=16000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=True,
        )
        assert _detect_env(snap) == "server"

    def test_low_ram_is_embedded(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=1000,
            total_ram_mb=1400,
            cpu_cores=2,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=1024,
            has_display=False,
            in_container=False,
        )
        assert _detect_env(snap) == "embedded"

    def test_display_is_pc(self, snapshot_laptop: ResourceSnapshot) -> None:
        assert _detect_env(snapshot_laptop) == "pc"

    def test_headless_no_container_is_server(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        assert _detect_env(snapshot_server) == "server"


# ── compute_scale — validation ───────────────────────────────────────────


class TestComputeScaleValidation:
    def test_invalid_profile_raises(self, snapshot_server: ResourceSnapshot) -> None:
        with pytest.raises(ValueError, match="Unknown profile"):
            compute_scale(
                "turbo",  # type: ignore[arg-type]
                snapshot=snapshot_server,
            )

    def test_invalid_env_raises(self, snapshot_server: ResourceSnapshot) -> None:
        with pytest.raises(ValueError, match="Unknown env"):
            compute_scale(
                "balanced",
                env="mars",  # type: ignore[arg-type]
                snapshot=snapshot_server,
            )

    def test_low_ram_raises(self, snapshot_low_ram: ResourceSnapshot) -> None:
        with pytest.raises(InsufficientResourcesError, match="Insufficient RAM"):
            compute_scale("balanced", snapshot=snapshot_low_ram)

    def test_low_fd_raises(self, snapshot_low_fd: ResourceSnapshot) -> None:
        with pytest.raises(InsufficientResourcesError, match="File-descriptor"):
            compute_scale("balanced", snapshot=snapshot_low_fd)


# ── compute_scale — profiles ─────────────────────────────────────────────


class TestComputeScaleProfiles:
    def test_minimal_always_1_browser_2_tabs(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        report = compute_scale("minimal", snapshot=snapshot_server)
        assert report.browsers == 1
        assert report.tabs_per_browser == 2
        assert report.headless is True
        assert report.tab_max_idle_secs == 20

    def test_balanced_caps_tabs_at_60(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        assert report.total_tabs <= 60
        assert report.browsers >= 1
        assert report.browsers <= 4

    def test_advanced_uses_30_tabs_per_browser(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        report = compute_scale("advanced", snapshot=snapshot_server)
        assert report.tabs_per_browser == 30
        assert report.headless is True

    def test_balanced_with_display_not_forced_headless(
        self, snapshot_laptop: ResourceSnapshot
    ) -> None:
        report = compute_scale("balanced", snapshot=snapshot_laptop)
        # balanced doesn't force headless + display present
        assert report.headless is False

    def test_minimal_forces_headless_even_with_display(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=8000,
            total_ram_mb=16000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=True,
            in_container=False,
        )
        report = compute_scale("minimal", snapshot=snap)
        assert report.headless is True


# ── compute_scale — warnings & downgrades ────────────────────────────────


class TestComputeScaleWarnings:
    def test_swap_downgrades_advanced_to_balanced(
        self, snapshot_swapping_server: ResourceSnapshot
    ) -> None:
        report = compute_scale("advanced", snapshot=snapshot_swapping_server)
        assert report.profile == "balanced"
        assert any("Swap is active" in w for w in report.warnings)

    def test_high_cpu_load_warns(self, snapshot_high_load: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_high_load)
        assert any("High CPU load" in w for w in report.warnings)

    def test_no_warnings_when_healthy(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        assert report.warnings == []


# ── compute_scale — env override ─────────────────────────────────────────


class TestComputeScaleEnvOverride:
    def test_env_auto_detects(self, snapshot_laptop: ResourceSnapshot) -> None:
        report = compute_scale("balanced", env="auto", snapshot=snapshot_laptop)
        assert report.detected_env == "pc"

    def test_env_forced_server(self, snapshot_laptop: ResourceSnapshot) -> None:
        report = compute_scale("balanced", env="server", snapshot=snapshot_laptop)
        assert report.detected_env == "server"

    def test_env_forced_embedded(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", env="embedded", snapshot=snapshot_server)
        assert report.detected_env == "embedded"


# ── _compute_tabs internals ──────────────────────────────────────────────


class TestComputeTabs:
    def test_minimal_fixed(self, snapshot_server: ResourceSnapshot) -> None:
        params = _PROFILE_PARAMS["minimal"]
        browsers, tabs = _compute_tabs("minimal", snapshot_server, params)
        assert (browsers, tabs) == (1, 2)

    def test_balanced_respects_fd_limit(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=8000,
            total_ram_mb=16000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=600,
            has_display=False,
            in_container=False,
        )
        params = _PROFILE_PARAMS["balanced"]
        browsers, tabs = _compute_tabs("balanced", snap, params)
        assert browsers >= 1
        assert tabs >= 1

    def test_advanced_scales_with_ram(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=64_000,
            total_ram_mb=128_000,
            cpu_cores=32,
            load_avg_1m=1.0,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=False,
        )
        params = _PROFILE_PARAMS["advanced"]
        browsers, tabs = _compute_tabs("advanced", snap, params)
        assert tabs == 30  # capped at max_tabs_per_browser
        assert browsers >= 2


# ── ScaleReport methods ──────────────────────────────────────────────────


class TestScaleReport:
    def test_total_tabs(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        assert report.total_tabs == report.browsers * report.tabs_per_browser

    def test_to_pool_config(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        cfg = report.to_pool_config()
        assert cfg.browsers == report.browsers
        assert cfg.tabs_per_browser == report.tabs_per_browser
        assert cfg.tab_max_idle_secs == report.tab_max_idle_secs
        assert cfg.browser.headless == report.headless

    def test_to_dict_keys(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        d = report.to_dict()
        assert "detected_env" in d
        assert "profile" in d
        assert "browsers" in d
        assert "tabs_per_browser" in d
        assert "total_tabs" in d
        assert "headless" in d
        assert "warnings" in d
        assert "snapshot" in d
        assert "env_vars" in d

    def test_to_dict_json_serialisable(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        text = json.dumps(report.to_dict())
        assert isinstance(text, str)

    def test_to_dict_env_vars(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        env = report.to_dict()["env_vars"]
        assert isinstance(env, dict)
        assert "SCALE_PROFILE" in env
        assert "BROWSER_COUNT" in env
        assert "TABS_PER_BROWSER" in env
        assert "CHROME_WS_URLS" in env

    def test_print_report_no_crash(
        self,
        snapshot_server: ResourceSnapshot,
        capsys: pytest.CaptureFixture[str],
    ) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        report.print_report()
        out = capsys.readouterr().out
        assert "VoidCrawl Scale Report" in out

    def test_print_report_shows_cgroup(
        self,
        snapshot_container_cgroup: ResourceSnapshot,
        capsys: pytest.CaptureFixture[str],
    ) -> None:
        report = compute_scale("balanced", snapshot=snapshot_container_cgroup)
        report.print_report()
        out = capsys.readouterr().out
        assert "cgroup" in out.lower()


# ── generate_supervisord_conf ────────────────────────────────────────────


class TestGenerateSupervisordConf:
    def test_generates_correct_program_count(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report)
        assert conf.count("[program:chrome-debug-") == report.browsers

    def test_headless_flag_present(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("minimal", snapshot=snapshot_server)
        assert report.headless is True
        conf = generate_supervisord_conf(report)
        assert "--headless=new" in conf

    def test_headful_no_headless_flag(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=8000,
            total_ram_mb=16000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=True,
            in_container=False,
        )
        report = compute_scale("balanced", snapshot=snap)
        assert report.headless is False
        conf = generate_supervisord_conf(report)
        assert "--headless=new" not in conf

    def test_port_assignment(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report, base_port=9222)
        for i in range(report.browsers):
            assert f"--remote-debugging-port={9222 + i}" in conf

    def test_user_data_dir_isolation(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report)
        for i in range(report.browsers):
            assert f"--user-data-dir=/tmp/chrome-profile-{i + 1}" in conf

    def test_custom_base_port(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("minimal", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report, base_port=5000)
        assert "--remote-debugging-port=5000" in conf

    def test_supervisord_section_present(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report)
        assert "[supervisord]" in conf
        assert "nodaemon=true" in conf

    def test_chrome_flags_present(self, snapshot_server: ResourceSnapshot) -> None:
        report = compute_scale("balanced", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report)
        for flag in [
            "--no-sandbox",
            "--remote-allow-origins=*",
            "--disable-dev-shm-usage",
            "--no-service-autorun",
            "--password-store=basic",
        ]:
            assert flag in conf
        if report.headless:
            assert "--disable-blink-features=AutomationControlled" in conf
        for removed in [
            "--no-zygote",
            "--disable-background-networking",
        ]:
            assert removed not in conf

    def test_chrome_uses_hardware_gpu_not_swiftshader(
        self, snapshot_server: ResourceSnapshot
    ) -> None:
        # Hardware GPU via ANGLE, not `--disable-gpu` (which forces SwiftShader
        # software WebGL — a bot signal). See CAS-64.
        report = compute_scale("balanced", snapshot=snapshot_server)
        conf = generate_supervisord_conf(report)
        for flag in [
            "--enable-gpu",
            "--ignore-gpu-blocklist",
            "--use-angle=vulkan",
            "--disable-gpu-sandbox",
        ]:
            assert flag in conf
        # The software-rendering flag must be gone (guard against the substring
        # match with --disable-gpu-sandbox by checking for a flag boundary).
        assert "--disable-gpu " not in conf
        assert not conf.rstrip().endswith("--disable-gpu")


# ── Edge cases ───────────────────────────────────────────────────────────


class TestEdgeCases:
    def test_exactly_600mb_ram_succeeds(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=600,
            total_ram_mb=1024,
            cpu_cores=1,
            load_avg_1m=0.1,
            swap_used_mb=0,
            fd_soft_limit=1024,
            has_display=False,
            in_container=False,
        )
        report = compute_scale("minimal", snapshot=snap)
        assert report.browsers >= 1

    def test_exactly_256_fd_succeeds(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=2000,
            total_ram_mb=4000,
            cpu_cores=2,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=256,
            has_display=False,
            in_container=False,
        )
        report = compute_scale("minimal", snapshot=snap)
        assert report.browsers >= 1

    def test_cgroup_limits_effective_ram(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=16_000,
            total_ram_mb=32_000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=True,
            cgroup_mem_limit_mb=700,
        )
        report = compute_scale("minimal", snapshot=snap)
        assert report.browsers >= 1

    def test_cgroup_below_600mb_fails(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=16_000,
            total_ram_mb=32_000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=True,
            cgroup_mem_limit_mb=400,
        )
        with pytest.raises(InsufficientResourcesError):
            compute_scale("minimal", snapshot=snap)

    def test_swap_does_not_downgrade_minimal(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=4000,
            total_ram_mb=8000,
            cpu_cores=2,
            load_avg_1m=0.5,
            swap_used_mb=200,
            fd_soft_limit=65536,
            has_display=False,
            in_container=False,
        )
        report = compute_scale("minimal", snapshot=snap)
        assert report.profile == "minimal"

    def test_swap_does_not_downgrade_balanced(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=4000,
            total_ram_mb=8000,
            cpu_cores=2,
            load_avg_1m=0.5,
            swap_used_mb=200,
            fd_soft_limit=65536,
            has_display=False,
            in_container=False,
        )
        report = compute_scale("balanced", snapshot=snap)
        assert report.profile == "balanced"
