"""Static and generated-config checks for the CAS-243 headful runtime."""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import subprocess
from pathlib import Path

ROOT = Path(__file__).parents[1]
DOCKER = ROOT / "docker"
ENTRYPOINT = DOCKER / "entrypoint-headful.sh"
VIEWERCTL = DOCKER / "viewerctl.sh"


def _generate_supervisor_config(
    tmp_path: Path, no_sandbox: str | None = None
) -> tuple[subprocess.CompletedProcess[str], str]:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    supervisor = bin_dir / "supervisord"
    supervisor.write_text("#!/bin/sh\nexit 0\n")
    supervisor.chmod(0o755)

    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{bin_dir}:{env['PATH']}",
            "RUNTIME_DIR": str(tmp_path / "runtime"),
            "XDG_RUNTIME_DIR": str(tmp_path / "runtime" / "xdg"),
            "CHROME_PROFILES_DIR": str(tmp_path / "profiles"),
            "BROWSER_COUNT": "1",
            "CDP_PORT_BASE": "19222",
            "VNC_PORT_BASE": "5900",
            "VNC_WIDTH": "1280",
            "VNC_HEIGHT": "720",
            "WLR_RENDERER": "pixman",
        }
    )
    if no_sandbox is None:
        env.pop("CHROME_NO_SANDBOX", None)
    else:
        env["CHROME_NO_SANDBOX"] = no_sandbox

    result = subprocess.run(
        ["bash", str(ENTRYPOINT)],
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    config_path = tmp_path / "runtime" / "config" / "supervisord.conf"
    config = config_path.read_text() if config_path.exists() else ""
    return result, config


def _chrome_command(config: str) -> list[str]:
    line = next(
        line for line in config.splitlines() if line.startswith("command=google-chrome")
    )
    return shlex.split(line.removeprefix("command="))


def test_headful_chrome_is_sandboxed_by_default(tmp_path: Path) -> None:
    result, config = _generate_supervisor_config(tmp_path)

    assert result.returncode == 0, result.stderr
    assert "--no-sandbox" not in _chrome_command(config)
    assert "WARNING: Chrome sandbox disabled" not in result.stderr


def test_no_sandbox_is_an_explicit_warned_compatibility_mode(tmp_path: Path) -> None:
    result, config = _generate_supervisor_config(tmp_path, "1")

    assert result.returncode == 0, result.stderr
    assert "--no-sandbox" in _chrome_command(config)
    assert "WARNING: Chrome sandbox disabled" in result.stderr


def test_invalid_no_sandbox_value_fails_closed(tmp_path: Path) -> None:
    result, config = _generate_supervisor_config(tmp_path, "sometimes")

    assert result.returncode == 64
    assert not config
    assert "CHROME_NO_SANDBOX must be 0 or 1" in result.stderr


def test_viewer_is_inactive_and_cannot_open_when_disabled(tmp_path: Path) -> None:
    env = os.environ.copy()
    env.update({"RUNTIME_DIR": str(tmp_path), "VIEWER_MODE": "disabled"})

    status = subprocess.run(
        ["bash", str(VIEWERCTL), "status"],
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    opened = subprocess.run(
        ["bash", str(VIEWERCTL), "open", "--browser", "1"],
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert status.returncode == 0
    assert status.stdout.strip() == "viewer: inactive"
    assert opened.returncode == 77
    assert "viewer mode is disabled" in opened.stderr


def test_compose_keeps_hardening_and_applies_chrome_seccomp_profile() -> None:
    compose = (DOCKER / "docker-compose.headful.yml").read_text()

    assert "cap_drop:\n    - ALL" in compose
    assert "no-new-privileges:true" in compose
    assert "seccomp=./seccomp-chrome.json" in compose
    assert "seccomp=unconfined" not in compose
    assert "CAP_SYS_ADMIN" not in compose
    assert 'CHROME_NO_SANDBOX: "${CHROME_NO_SANDBOX:-0}"' in compose
    assert 'VIEWER_MODE: "${VIEWER_MODE:-disabled}"' in compose


def test_chrome_seccomp_delta_is_exactly_the_reviewed_bootstrap_allowlist() -> None:
    profile = json.loads((DOCKER / "seccomp-chrome.json").read_text())

    expected = {
        "arch_prctl",
        "chroot",
        "clone",
        "fanotify_init",
        "mlock",
        "mlockall",
        "name_to_handle_at",
        "open_by_handle_at",
        "setdomainname",
        "sethostname",
        "setns",
        "unshare",
        "vhangup",
    }
    delta = [
        rule
        for rule in profile["syscalls"]
        if rule.get("comment", "").startswith("Chrome Linux sandbox bootstrap")
    ]

    assert profile["defaultAction"] == "SCMP_ACT_ERRNO"
    assert len(delta) == 1
    assert delta[0]["action"] == "SCMP_ACT_ALLOW"
    assert set(delta[0]["names"]) == expected
    assert "includes" not in delta[0]
    assert "excludes" not in delta[0]

    # Pin every other rule to the cited Moby baseline so a future edit cannot
    # broaden the profile outside the one conspicuous, reviewed Chrome delta.
    baseline = profile | {
        "syscalls": [rule for rule in profile["syscalls"] if rule is not delta[0]]
    }
    canonical = json.dumps(baseline, sort_keys=True, separators=(",", ":")).encode()
    assert hashlib.sha256(canonical).hexdigest() == (
        "9da637d2ab0a204fcbd91bd88f1be9e004a3acab61c571a9f5b8870e588a17d2"
    )
