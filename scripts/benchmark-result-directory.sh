#!/usr/bin/env bash
set -euo pipefail
class=${1:?measurement class required}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
if command -v jj >/dev/null 2>&1 && change=$(jj log -r @ --no-graph -T 'change_id ++ "\n"' 2>/dev/null | head -n1) && [[ -n $change ]]; then
  printf '%s/voidcrawl-benchmarks/results/by-change/jj/%s/%s\n' "$root" "$change" "$class"
else
  printf '%s/voidcrawl-benchmarks/results/by-change/git/%s/%s\n' "$root" "$(git rev-parse HEAD)" "$class"
fi
