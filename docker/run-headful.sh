#!/usr/bin/env bash
# Thin wrapper: detect GPU vendor, then hand off to `docker compose`.
#
# All runtime configuration lives in docker-compose.headful.yml + .env.
# This script only exists to auto-pick the right compose profile based on
# what's actually plugged into the host.
#
# Usage:
#   ./docker/run-headful.sh                 # auto-detect, foreground
#   ./docker/run-headful.sh -d              # auto-detect, detached
#   COMPOSE_PROFILES=amd docker compose -f docker/docker-compose.headful.yml up
#     # ...is the equivalent compose-native invocation. Use that in CI.
#
# Override resolution via .env or inline:
#   VNC_WIDTH=2560 VNC_HEIGHT=1440 ./docker/run-headful.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.headful.yml"

detect_gpu() {
    if [ -e /dev/dri/renderD128 ]; then
        local driver
        driver=$(basename "$(readlink -f /sys/class/drm/renderD128/device/driver)" 2>/dev/null || echo "")
        case "$driver" in
            amdgpu)  echo amd;    return ;;
            i915|xe) echo intel;  return ;;
            nvidia)  echo nvidia; return ;;
        esac
    fi
    [ -e /dev/nvidia0 ] && { echo nvidia; return; }
    echo cpu
}

: "${COMPOSE_PROFILES:=$(detect_gpu)}"
export COMPOSE_PROFILES

echo "[run-headful] profile=$COMPOSE_PROFILES  (override via COMPOSE_PROFILES=...)"
exec docker compose -f "$COMPOSE_FILE" up "$@"
