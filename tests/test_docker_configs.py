"""Static Docker Chrome launch-surface checks."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADFUL_SUPERVISORD = ROOT / "docker" / "config" / "supervisord-headful.conf"
HEADLESS_SUPERVISORD = ROOT / "docker" / "config" / "supervisord.conf"


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


def test_headless_config_keeps_headless_specific_webdriver_suppression() -> None:
    conf = HEADLESS_SUPERVISORD.read_text()

    assert "--headless=new" in conf
    assert "--disable-blink-features=AutomationControlled" in conf
    assert "--remote-allow-origins=*" in conf
    assert "--disable-infobars" not in conf
    assert "--disable-background-networking" not in conf
    assert "--disable-renderer-backgrounding" not in conf
