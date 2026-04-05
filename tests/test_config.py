"""Unit tests for BrowserConfig, PoolConfig, and BrowserSession/BrowserPool
configuration logic (no browser required)."""

from __future__ import annotations

import os
from unittest.mock import patch

import pytest

from voidcrawl import BrowserConfig, BrowserPool, BrowserSession, PoolConfig
from voidcrawl.scale import ResourceSnapshot

# ── BrowserConfig ────────────────────────────────────────────────────────


class TestBrowserConfig:
    def test_defaults(self) -> None:
        cfg = BrowserConfig()
        assert cfg.headless is True
        assert cfg.stealth is True
        assert cfg.no_sandbox is False
        assert cfg.proxy is None
        assert cfg.chrome_executable is None
        assert cfg.extra_args == []
        assert cfg.ws_url is None
        assert cfg.debug is False
        assert cfg.stepping is True
        assert cfg.highlight is True
        assert cfg.step_delay == 0.3

    def test_custom_values(self) -> None:
        cfg = BrowserConfig(
            headless=False,
            stealth=False,
            no_sandbox=True,
            proxy="http://proxy:8080",
            chrome_executable="/usr/bin/chromium",
            extra_args=["--disable-extensions"],
            ws_url="ws://localhost:9222",
            debug=True,
            stepping=False,
            highlight=False,
            step_delay=1.0,
        )
        assert cfg.headless is False
        assert cfg.no_sandbox is True
        assert cfg.proxy == "http://proxy:8080"
        assert cfg.ws_url == "ws://localhost:9222"
        assert cfg.debug is True
        assert cfg.stepping is False
        assert cfg.step_delay == 1.0

    def test_model_dump(self) -> None:
        cfg = BrowserConfig()
        d = cfg.model_dump()
        assert isinstance(d, dict)
        assert "headless" in d
        assert "stealth" in d

    def test_extra_args_immutable_default(self) -> None:
        cfg1 = BrowserConfig()
        cfg2 = BrowserConfig()
        cfg1.extra_args.append("--foo")
        assert "--foo" not in cfg2.extra_args


# ── PoolConfig ───────────────────────────────────────────────────────────


class TestPoolConfig:
    def test_defaults(self) -> None:
        cfg = PoolConfig()
        assert cfg.browsers == 1
        assert cfg.tabs_per_browser == 4
        assert cfg.tab_max_uses == 50
        assert cfg.tab_max_idle_secs == 60
        assert cfg.acquire_timeout_secs == 30
        assert cfg.auto_evict is True
        assert cfg.chrome_ws_urls == []
        assert isinstance(cfg.browser, BrowserConfig)

    def test_custom_values(self) -> None:
        browser = BrowserConfig(headless=False)
        cfg = PoolConfig(
            browsers=3,
            tabs_per_browser=10,
            tab_max_uses=100,
            tab_max_idle_secs=120,
            acquire_timeout_secs=15,
            auto_evict=False,
            chrome_ws_urls=["http://localhost:9222"],
            browser=browser,
        )
        assert cfg.browsers == 3
        assert cfg.tabs_per_browser == 10
        assert cfg.acquire_timeout_secs == 15
        assert cfg.auto_evict is False
        assert cfg.chrome_ws_urls == ["http://localhost:9222"]
        assert cfg.browser.headless is False

    def test_model_dump(self) -> None:
        cfg = PoolConfig()
        d = cfg.model_dump()
        assert isinstance(d, dict)
        assert "browsers" in d
        assert "browser" in d


# ── PoolConfig.from_profile ─────────────────────────────────────────────


class TestPoolConfigFromProfile:
    def test_balanced_profile(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=8000,
            total_ram_mb=16000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=False,
        )
        with patch("voidcrawl.scale.detect_resources", return_value=snap):
            cfg = PoolConfig.from_profile("balanced")
        assert cfg.browsers >= 1
        assert cfg.tabs_per_browser >= 1

    def test_minimal_profile(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=2000,
            total_ram_mb=4000,
            cpu_cores=2,
            load_avg_1m=0.3,
            swap_used_mb=0,
            fd_soft_limit=1024,
            has_display=False,
            in_container=False,
        )
        with patch("voidcrawl.scale.detect_resources", return_value=snap):
            cfg = PoolConfig.from_profile("minimal")
        assert cfg.browsers == 1
        assert cfg.tabs_per_browser == 2

    def test_profile_propagates_headless(self) -> None:
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
        with patch("voidcrawl.scale.detect_resources", return_value=snap):
            cfg = PoolConfig.from_profile("balanced")
        assert cfg.browser.headless is False  # balanced + display = headful


# ── PoolConfig.from_docker ──────────────────────────────────────────────


class TestPoolConfigFromDocker:
    def test_headless_defaults(self) -> None:
        cfg = PoolConfig.from_docker(check=False)
        assert cfg.chrome_ws_urls == ["http://localhost:9222", "http://localhost:9223"]
        assert cfg.browsers == 2

    def test_headful_defaults(self) -> None:
        cfg = PoolConfig.from_docker(headful=True, check=False)
        assert cfg.chrome_ws_urls == [
            "http://localhost:19222",
            "http://localhost:19223",
        ]
        assert cfg.browsers == 2

    def test_custom_host(self) -> None:
        cfg = PoolConfig.from_docker(host="chrome.local", check=False)
        assert all("chrome.local" in url for url in cfg.chrome_ws_urls)

    def test_custom_ports(self) -> None:
        cfg = PoolConfig.from_docker(ports=[5000, 5001, 5002], check=False)
        assert len(cfg.chrome_ws_urls) == 3
        assert cfg.browsers == 3
        assert "http://localhost:5000" in cfg.chrome_ws_urls

    def test_tabs_per_browser(self) -> None:
        cfg = PoolConfig.from_docker(tabs_per_browser=8, check=False)
        assert cfg.tabs_per_browser == 8

    def test_check_raises_when_unreachable(self) -> None:
        with (
            patch(
                "voidcrawl._first_unreachable",
                return_value="http://localhost:9222",
            ),
            pytest.raises(RuntimeError, match="Cannot reach Chrome"),
        ):
            PoolConfig.from_docker(check=True)

    def test_check_passes_when_reachable(self) -> None:
        with patch("voidcrawl._first_unreachable", return_value=None):
            cfg = PoolConfig.from_docker(check=True)
            assert len(cfg.chrome_ws_urls) == 2


# ── PoolConfig.from_env ─────────────────────────────────────────────────


class TestPoolConfigFromEnv:
    def test_defaults_no_env(self) -> None:
        env: dict[str, str] = {}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browsers == 1
        assert cfg.tabs_per_browser == 4
        assert cfg.tab_max_uses == 50
        assert cfg.tab_max_idle_secs == 60
        assert cfg.acquire_timeout_secs == 30
        assert cfg.auto_evict is True
        assert cfg.chrome_ws_urls == []

    def test_ws_urls(self) -> None:
        env = {"CHROME_WS_URLS": "http://a:9222,http://b:9222"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.chrome_ws_urls == ["http://a:9222", "http://b:9222"]
        assert cfg.browsers == 2  # derived from len(urls)

    def test_browser_count(self) -> None:
        env = {"BROWSER_COUNT": "3"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browsers == 3

    def test_browser_count_ignored_with_ws_urls(self) -> None:
        env = {"CHROME_WS_URLS": "http://a:9222", "BROWSER_COUNT": "5"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browsers == 1  # from len(urls), not BROWSER_COUNT

    def test_tabs_per_browser(self) -> None:
        env = {"TABS_PER_BROWSER": "10"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.tabs_per_browser == 10

    def test_tab_max_uses(self) -> None:
        env = {"TAB_MAX_USES": "100"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.tab_max_uses == 100

    def test_tab_max_idle_secs(self) -> None:
        env = {"TAB_MAX_IDLE_SECS": "120"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.tab_max_idle_secs == 120

    def test_acquire_timeout_secs(self) -> None:
        env = {"ACQUIRE_TIMEOUT_SECS": "10"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.acquire_timeout_secs == 10

    def test_auto_evict_disabled(self) -> None:
        env = {"AUTO_EVICT": "0"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.auto_evict is False

    def test_auto_evict_enabled(self) -> None:
        env = {"AUTO_EVICT": "1"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.auto_evict is True

    def test_no_sandbox(self) -> None:
        env = {"CHROME_NO_SANDBOX": "1"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browser.no_sandbox is True

    def test_headful(self) -> None:
        env = {"CHROME_HEADLESS": "0"}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browser.headless is False

    def test_headless_default(self) -> None:
        env: dict[str, str] = {}
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browser.headless is True

    def test_scale_profile_delegates(self) -> None:
        snap = ResourceSnapshot(
            free_ram_mb=8000,
            total_ram_mb=16000,
            cpu_cores=4,
            load_avg_1m=0.5,
            swap_used_mb=0,
            fd_soft_limit=65536,
            has_display=False,
            in_container=False,
        )
        env = {"SCALE_PROFILE": "minimal"}
        with (
            patch.dict(os.environ, env, clear=True),
            patch("voidcrawl.scale.detect_resources", return_value=snap),
        ):
            cfg = PoolConfig.from_env()
        assert cfg.browsers == 1
        assert cfg.tabs_per_browser == 2

    def test_full_docker_env(self) -> None:
        """Simulate the env vars that docker-compose.yml sets."""
        env = {
            "CHROME_WS_URLS": "http://localhost:9222,http://localhost:9223",
            "BROWSER_COUNT": "2",
            "TABS_PER_BROWSER": "4",
            "TAB_MAX_USES": "50",
            "TAB_MAX_IDLE_SECS": "60",
            "ACQUIRE_TIMEOUT_SECS": "30",
            "CHROME_NO_SANDBOX": "1",
        }
        with patch.dict(os.environ, env, clear=True):
            cfg = PoolConfig.from_env()
        assert cfg.browsers == 2
        assert cfg.tabs_per_browser == 4
        assert cfg.tab_max_uses == 50
        assert cfg.acquire_timeout_secs == 30
        assert cfg.browser.no_sandbox is True
        assert len(cfg.chrome_ws_urls) == 2


# ── BrowserSession (config-level only) ──────────────────────────────────


class TestBrowserSessionConfig:
    def test_default_config(self) -> None:
        session = BrowserSession()
        assert session._config.headless is True
        assert session._config.stealth is True

    def test_custom_config(self) -> None:
        cfg = BrowserConfig(headless=False, debug=True)
        session = BrowserSession(cfg)
        assert session._config.headless is False
        assert session._config.debug is True

    def test_repr_headless(self) -> None:
        session = BrowserSession()
        assert "headless" in repr(session)

    def test_repr_headful(self) -> None:
        session = BrowserSession(BrowserConfig(headless=False))
        assert "headful" in repr(session)

    def test_repr_ws(self) -> None:
        session = BrowserSession(BrowserConfig(ws_url="ws://localhost:9222"))
        assert "ws" in repr(session)

    async def test_not_started_raises(self) -> None:
        session = BrowserSession()
        with pytest.raises(RuntimeError, match="not started"):
            await session.new_page("http://example.com")

    async def test_version_not_started_raises(self) -> None:
        session = BrowserSession()
        with pytest.raises(RuntimeError, match="not started"):
            await session.version()


# ── BrowserPool (config-level only) ─────────────────────────────────────


class TestBrowserPoolConfig:
    def test_repr(self) -> None:
        pool = BrowserPool(PoolConfig(browsers=2, tabs_per_browser=8))
        assert "browsers=2" in repr(pool)
        assert "tabs_per_browser=8" in repr(pool)

    def test_acquire_not_started_raises(self) -> None:
        pool = BrowserPool(PoolConfig())
        with pytest.raises(RuntimeError, match="not started"):
            pool.acquire()

    async def test_warmup_not_started_raises(self) -> None:
        pool = BrowserPool(PoolConfig())
        with pytest.raises(RuntimeError, match="not started"):
            await pool.warmup()
