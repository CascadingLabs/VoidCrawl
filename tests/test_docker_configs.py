"""Static checks for the Docker container's DNS override hook.

``docker/ensure-dns.sh`` writes ``/etc/resolv.conf`` from environment variables,
so its input validation is the only thing standing between an operator typo and
a malformed resolver config inside the container. These run the real script
against a temp path — no container needed.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ENSURE_DNS = ROOT / "docker" / "ensure-dns.sh"


def _run_ensure_dns(
    resolv: Path, **env_overrides: str
) -> subprocess.CompletedProcess[str]:
    env = {
        **os.environ,
        "VOIDCRAWL_RESOLV_CONF_PATH": str(resolv),
        **env_overrides,
    }
    return subprocess.run(
        [str(ENSURE_DNS)],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def test_dns_override_is_a_noop_when_unset(tmp_path: Path) -> None:
    """The hook runs unconditionally at container start, so the common case —
    no override configured — must leave resolv.conf completely alone."""
    resolv = tmp_path / "resolv.conf"
    result = _run_ensure_dns(resolv, VOIDCRAWL_DNS_SERVERS="")

    assert result.returncode == 0
    assert not resolv.exists()


def test_dns_override_rejects_multiline_search_values(tmp_path: Path) -> None:
    """A newline in a value would let an operator inject an arbitrary extra
    resolv.conf directive. Reject rather than write a partial file."""
    resolv = tmp_path / "resolv.conf"
    result = _run_ensure_dns(
        resolv,
        VOIDCRAWL_DNS_SERVERS="1.1.1.1",
        VOIDCRAWL_DNS_SEARCH="example.com\noptions bad",
    )

    assert result.returncode == 2
    assert "invalid VOIDCRAWL_DNS_SEARCH value" in result.stderr
    assert not resolv.exists()


def test_dns_override_rejects_non_address_nameserver(tmp_path: Path) -> None:
    resolv = tmp_path / "resolv.conf"
    result = _run_ensure_dns(resolv, VOIDCRAWL_DNS_SERVERS="1.1.1.1,not-an-address")

    assert result.returncode == 2
    assert "invalid nameserver" in result.stderr
    assert not resolv.exists()


def test_dns_override_writes_valid_search_and_options(tmp_path: Path) -> None:
    resolv = tmp_path / "resolv.conf"
    result = _run_ensure_dns(
        resolv,
        VOIDCRAWL_DNS_SERVERS="1.1.1.1,8.8.8.8",
        VOIDCRAWL_DNS_SEARCH="example.com internal.local",
        VOIDCRAWL_DNS_OPTIONS="timeout:1 attempts:2 rotate",
    )

    assert result.returncode == 0
    assert resolv.read_text() == (
        "nameserver 1.1.1.1\n"
        "nameserver 8.8.8.8\n"
        "search example.com internal.local\n"
        "options timeout:1 attempts:2 rotate\n"
    )


def test_dns_override_accepts_whitespace_separated_servers(tmp_path: Path) -> None:
    """The documented format is comma-separated, but the script also splits on
    whitespace — both are advertised in docker/.env.example."""
    resolv = tmp_path / "resolv.conf"
    result = _run_ensure_dns(resolv, VOIDCRAWL_DNS_SERVERS="1.1.1.1 8.8.8.8")

    assert result.returncode == 0
    assert resolv.read_text() == (
        "nameserver 1.1.1.1\nnameserver 8.8.8.8\noptions timeout:2 attempts:3 rotate\n"
    )
