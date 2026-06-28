"""Static Docker Chrome launch-surface checks."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADFUL_SUPERVISORD = ROOT / "docker" / "config" / "supervisord-headful.conf"
HEADLESS_SUPERVISORD = ROOT / "docker" / "config" / "supervisord.conf"
ENSURE_DNS = ROOT / "docker" / "ensure-dns.sh"


def test_headful_config_keeps_human_parity_launch_surface() -> None:
    conf = HEADFUL_SUPERVISORD.read_text()

    for expected in [
        "--ozone-platform=wayland",
        "--enable-features=UseOzonePlatform",
        "--no-first-run",
        "--no-service-autorun",
        "--no-default-browser-check",
        "--no-pings",
        "--password-store=basic",
        "--homepage=about:blank",
    ]:
        assert expected in conf

    for noisy in [
        "--disable-blink-features=AutomationControlled",
        "--disable-infobars",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-background-timer-throttling",
        "--disable-renderer-backgrounding",
        "--disable-ipc-flooding-protection",
        "--disable-features=PaintHolding,DeferRendererTasksAfterInput",
    ]:
        assert noisy not in conf


def test_dns_override_rejects_multiline_search_values(tmp_path: Path) -> None:
    resolv = tmp_path / "resolv.conf"
    env = {
        **os.environ,
        "VOIDCRAWL_DNS_SERVERS": "1.1.1.1",
        "VOIDCRAWL_DNS_SEARCH": "example.com\noptions bad",
        "VOIDCRAWL_RESOLV_CONF_PATH": str(resolv),
    }

    result = subprocess.run(
        [str(ENSURE_DNS)],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 2
    assert "invalid VOIDCRAWL_DNS_SEARCH value" in result.stderr
    assert not resolv.exists()


def test_dns_override_writes_valid_search_and_options(tmp_path: Path) -> None:
    resolv = tmp_path / "resolv.conf"
    env = {
        **os.environ,
        "VOIDCRAWL_DNS_SERVERS": "1.1.1.1,8.8.8.8",
        "VOIDCRAWL_DNS_SEARCH": "example.com internal.local",
        "VOIDCRAWL_DNS_OPTIONS": "timeout:1 attempts:2 rotate",
        "VOIDCRAWL_RESOLV_CONF_PATH": str(resolv),
    }

    result = subprocess.run(
        [str(ENSURE_DNS)],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 0
    assert resolv.read_text() == (
        "nameserver 1.1.1.1\n"
        "nameserver 8.8.8.8\n"
        "search example.com internal.local\n"
        "options timeout:1 attempts:2 rotate\n"
    )


def test_headless_config_keeps_headless_specific_webdriver_suppression() -> None:
    conf = HEADLESS_SUPERVISORD.read_text()

    assert "--headless=new" in conf
    assert "--disable-blink-features=AutomationControlled" in conf
    assert "--remote-allow-origins=*" in conf
    assert "--disable-infobars" not in conf
    assert "--disable-background-networking" not in conf
    assert "--disable-renderer-backgrounding" not in conf
