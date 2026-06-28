#!/usr/bin/env bash
# VoidCrawl headless Chrome entrypoint.
# Generates a supervisord.conf from voidcrawl.scale at container start so the
# Chrome farm is sized to match available memory, CPU, and file-descriptor limits.
#
# Environment variables:
#   SCALE_PROFILE   — minimal | balanced (default) | advanced
#   CHROME_WS_URLS  — if already set, skip scale computation (passthrough mode)
#   CDP_PORT_BASE   — first Chrome --remote-debugging-port (default 9222).
#                     Browser N listens on base+N. Override when 9222/9223
#                     are taken on the host — they're not privileged ports.
#   CDP_PORT_1/2    — static-mode fallbacks used by supervisord.conf when
#                     scale mode is skipped; default to base+0 / base+1.
set -euo pipefail

SCALE_PROFILE="${SCALE_PROFILE:-balanced}"
CDP_PORT_BASE="${CDP_PORT_BASE:-9222}"
# Backfill CDP_PORT_1/2 from CDP_PORT_BASE so `supervisord.conf` always
# resolves `%(ENV_CDP_PORT_1)s` / `%(ENV_CDP_PORT_2)s`, even on the
# static-config fast paths below.
export CDP_PORT_1="${CDP_PORT_1:-${CDP_PORT_BASE}}"
export CDP_PORT_2="${CDP_PORT_2:-$((CDP_PORT_BASE + 1))}"
CONF_PATH=/tmp/supervisord-dynamic.conf

# ── DNS override (optional) ──────────────────────────────────────────────
/usr/local/bin/ensure-dns.sh

# ── Chrome profiles ──────────────────────────────────────────────────────
# Each Chrome gets its own user-data-dir under CHROME_PROFILES_DIR. Default
# /tmp (ephemeral — wiped with the container). Set CHROME_PROFILES_DIR=/profiles
# (a mounted volume) to PERSIST logins, cookies, and Cloudflare clearance
# across restarts. The static supervisord config reads CHROME_PROFILE_DIR_1/2;
# the dynamic (scale) config builds its own paths from CHROME_PROFILES_DIR.
export CHROME_PROFILES_DIR="${CHROME_PROFILES_DIR:-/tmp}"
export CHROME_PROFILE_DIR_1="${CHROME_PROFILE_DIR_1:-${CHROME_PROFILES_DIR}/chrome-profile-1}"
export CHROME_PROFILE_DIR_2="${CHROME_PROFILE_DIR_2:-${CHROME_PROFILES_DIR}/chrome-profile-2}"
mkdir -p "$CHROME_PROFILE_DIR_1" "$CHROME_PROFILE_DIR_2"
echo "[profiles] base=$CHROME_PROFILES_DIR"

# Fast path: caller set CHROME_WS_URLS (e.g. PoolConfig.from_docker, or the
# static defaults in docker-compose.yml) → skip scale computation.
if [[ -n "${CHROME_WS_URLS:-}" ]]; then
    echo "[entrypoint] CHROME_WS_URLS is set — using static supervisord config"
    exec supervisord -c /etc/supervisor/conf.d/supervisord.conf
fi

# Fast path 2: voidcrawl Python package isn't installed in this image
# (published GHCR images are runtime-only — no Rust toolchain, no maturin).
# Fall back to the static 2-browser config. Users who want dynamic scaling
# can mount the voidcrawl wheel in or set CHROME_WS_URLS manually.
if ! python3 -c "import voidcrawl.scale" >/dev/null 2>&1; then
    echo "[entrypoint] voidcrawl.scale unavailable — using static supervisord config"
    exec supervisord -c /etc/supervisor/conf.d/supervisord.conf
fi

echo "[entrypoint] Scale profile: ${SCALE_PROFILE} (CDP base port: ${CDP_PORT_BASE})"

python3 - <<'PYEOF'
import os, sys
from voidcrawl.scale import compute_scale, generate_supervisord_conf, InsufficientResourcesError

profile = os.environ.get("SCALE_PROFILE", "balanced")
conf_path = os.environ.get("_VC_CONF_PATH", "/tmp/supervisord-dynamic.conf")
base_port = int(os.environ.get("CDP_PORT_BASE", "9222"))

try:
    report = compute_scale(profile=profile)
except InsufficientResourcesError as exc:
    print(f"[entrypoint] ERROR: {exc}", file=sys.stderr)
    sys.exit(1)

report.print_report()

# Create one profile dir per browser (matches generate_supervisord_conf).
profiles_dir = os.environ.get("CHROME_PROFILES_DIR", "/tmp").rstrip("/") or "/tmp"
for i in range(report.browsers):
    os.makedirs(f"{profiles_dir}/chrome-profile-{i + 1}", exist_ok=True)

conf = generate_supervisord_conf(report, base_port=base_port)
open(conf_path, "w").write(conf)

ws_urls = ",".join(
    f"http://localhost:{base_port + i}" for i in range(report.browsers)
)
# Write env vars to a file so the shell can source them
with open("/tmp/vc-env.sh", "w") as fh:
    fh.write(f"export CHROME_WS_URLS={ws_urls}\n")
    fh.write(f"export BROWSER_COUNT={report.browsers}\n")
    fh.write(f"export TABS_PER_BROWSER={report.tabs_per_browser}\n")
    fh.write(f"export TAB_MAX_IDLE_SECS={report.tab_max_idle_secs}\n")
    fh.write(f"export CHROME_HEADLESS={'1' if report.headless else '0'}\n")
PYEOF

# Export computed vars into the current shell environment
# shellcheck source=/dev/null
source /tmp/vc-env.sh

echo "[entrypoint] Launching ${BROWSER_COUNT} Chrome instance(s) via supervisord"
exec supervisord -c "${CONF_PATH}"
