#!/usr/bin/env bash
# Smoke detector for the CAS-210 occlusion regression.
#
# The latency fix rests on a routing chain that nothing else enforces:
#   --class=chrome-N  →  Chrome maps it to the Wayland app_id
#                     →  sway's `for_window [app_id="^chrome-N$"]` rule
#                     →  each window pinned to its own headless output.
# If any link breaks (a Chrome or sway upgrade changing how --class maps to
# app_id, the regex no longer matching, …) both Chromes fall through to the
# catch-all `[app_id=".*"] fullscreen` rule and pile onto ONE output. The
# occluded one's renderer then starves of frame callbacks and CDP ops stall —
# the exact >280s hang this work fixed. Crucially that failure is silent:
# CDP `/json/version` keeps answering. So this check is wired into the
# container HEALTHCHECK (not just startup) to make the regression fail loud.
#
# Exit 0 = each Chrome on its own output; non-zero = regression / not ready.
set -uo pipefail

export SWAYSOCK="${SWAYSOCK:-$(ls /tmp/xdg-runtime/sway-ipc.*.sock 2>/dev/null | head -1)}"
[ -S "${SWAYSOCK:-}" ] || { echo "[guard] no sway IPC socket yet"; exit 1; }

tree=$(swaymsg -t get_tree 2>/dev/null) || { echo "[guard] swaymsg failed"; exit 1; }

# Resolve which output each Chrome window currently sits under.
read -r out1 out2 < <(printf '%s' "$tree" | python3 -c '
import json, sys
loc = {}
def walk(node, out=None):
    if node.get("type") == "output":
        out = node.get("name")
    if node.get("app_id") in ("chrome-1", "chrome-2"):
        loc[node["app_id"]] = out
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        walk(child, out)
walk(json.load(sys.stdin))
print(loc.get("chrome-1", "?"), loc.get("chrome-2", "?"))
')

if [ "$out1" = "?" ] || [ "$out2" = "?" ]; then
    echo "[guard] not ready: chrome-1=$out1 chrome-2=$out2 (window not mapped yet)"
    exit 1
fi
if [ "$out1" = "$out2" ]; then
    echo "[guard] FAIL: both Chromes share output '$out1' — CAS-210 occlusion regressed"
    exit 1
fi
echo "[guard] OK: chrome-1=$out1 chrome-2=$out2"
