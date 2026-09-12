#!/usr/bin/env bash
# Run one isolated L2 class and atomically publish an immutable local artifact.
set -euo pipefail
kind=${1:?measurement class required}
case "$kind" in criterion|deterministic|allocations|process|heap) ;; *) echo "unknown class: $kind" >&2; exit 2;; esac
result_class=$kind
if test "$kind" = deterministic; then result_class=callgrind; fi
root=$(cd "$(dirname "$0")/.." && pwd); cd "$root"
class_root=$(scripts/benchmark-result-directory.sh "$result_class")
change_dir=$(dirname "$class_root")
# Capture this before creating generated result files: JJ identity may then change.
source_snapshot=$(jj log -r @ --no-graph -T 'commit_id' 2>/dev/null || git rev-parse HEAD)
fixture=voidcrawl-benchmarks/fixtures/index.html
python3 scripts/generate-benchmark-fixtures.py --check
mkdir -p "$class_root/runs"
stage=$(mktemp -d "$class_root/.${kind}.staging.XXXXXX")
published=false
cleanup() {
  status=$?
  if test "$status" -ne 0 && test -f "$stage/output.txt"; then
    printf 'benchmark class %s failed; captured output follows:\n' "$kind" >&2
    tail -n 200 "$stage/output.txt" >&2
  fi
  if ! "$published"; then rm -rf "$stage"; fi
  exit "$status"
}
trap cleanup EXIT INT TERM
version() { command -v "$1" >/dev/null 2>&1 && "$1" --version 2>&1 | head -n1 || printf unavailable; }
chrome_bin=$(command -v chromium || command -v chromium-browser || command -v google-chrome || true)
chrome_version=$({ test -n "$chrome_bin" && "$chrome_bin" --version 2>&1 | head -n1; } || printf unavailable)
cpu_model=$(LC_ALL=C lscpu 2>/dev/null | awk -F: '/Model name/{sub(/^ +/,"",$2);print $2;exit}')
gpu_model=$(lspci 2>/dev/null | grep -im1 -E 'vga|3d|display' || printf unavailable)
{
  printf 'class=%s\nsource_snapshot_commit=%s\nsource_snapshot_note=Captured before benchmark result writes; JJ identity may change when local generated artifacts are written.\n' "$result_class" "$source_snapshot"
  printf 'fixture_sha256=%s\nfixture_manifest=voidcrawl-benchmarks/fixtures/manifest.json\nfixture_checksum=voidcrawl-benchmarks/fixtures/SHA256SUMS\nnetwork_origin=loopback ephemeral HTTP server\n' "$(sha256sum "$fixture" | awk '{print $1}')"
  printf 'rust=%s\nchrome=%s\nkernel=%s\narch=%s\ncpu_model=%s\nlogical_cpu_count=%s\ninstalled_memory_kib=%s\ngpu_model=%s\ncgroup_v2=%s\nperf=%s\ngnu_time=%s\nvalgrind=%s\ngungraun_runner=%s\n' "$(rustc --version)" "$chrome_version" "$(uname -r)" "$(uname -m)" "${cpu_model:-unavailable}" "$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf unavailable)" "$(awk '/MemTotal:/{print $2;exit}' /proc/meminfo 2>/dev/null || printf unavailable)" "$gpu_model" "$(test -f /sys/fs/cgroup/cgroup.controllers && printf available || printf unavailable)" "$(version perf)" "$(version /usr/bin/time)" "$(version valgrind)" "$(version gungraun-runner)"
  printf 'viewport=browser default; dpr=browser default; pool_policy=warmup outside warm timing; matrix_hook=CONCURRENCY_MATRIX; raw_chromiumoxide_cdp=unavailable; raw_chromiumoxide_reason=no supported equivalent public lifecycle/byte-control path without duplicating private setup; unavailable_metrics_are_explicit=true\n'
} > "$stage/metadata.txt"
case "$kind" in
criterion)
  VOIDCRAWL_CONCURRENCY_EVIDENCE="$stage/concurrency.jsonl" CARGO_TARGET_DIR="$stage/target" cargo bench -p voidcrawl-benchmarks --bench criterion_browser -- --noplot >"$stage/output.txt" 2>&1
  cp -R "$stage/target/criterion" "$stage/criterion"
  ;;
deterministic) CARGO_TARGET_DIR="$stage/target" cargo bench -p voidcrawl-benchmarks --bench gungraun_sync >"$stage/output.txt" 2>&1; printf 'scope=synchronous fixture token counting only; browser/CDP is not profiled by Callgrind. runner=0.19 pinned by Cargo.lock\n' >> "$stage/metadata.txt" ;;
allocations) CARGO_TARGET_DIR="$stage/target" cargo bench -p voidcrawl-benchmarks --bench allocation_sync >"$stage/output.txt" 2>&1; printf 'scope=synchronous benchmark thread fixture token counting only; browser allocations are not claimed.\n' >> "$stage/metadata.txt" ;;
process)
  CARGO_TARGET_DIR="$stage/target" cargo build --release -p voidcrawl-benchmarks --bin profile_browser
  for workload in navigation_response navigation_load navigation_network_idle selector dom ax screenshot network_byte_control tab_create_close_reset controller_stop; do
    /usr/bin/time -v -o "$stage/time-${workload}.txt" "$stage/target/release/profile_browser" "$workload" >> "$stage/process.jsonl"
  done
  if command -v perf >/dev/null 2>&1 && perf stat -x, -e cycles,instructions -- true >/dev/null 2>&1; then perf stat -x, -e cycles,instructions -o "$stage/perf.csv" "$stage/target/release/profile_browser" navigation_response; else printf 'perf=unavailable_or_permission_denied\n' >> "$stage/metadata.txt"; fi ;;
heap)
  ulimit -c 0
  CARGO_TARGET_DIR="$stage/target" cargo build --release -p voidcrawl-benchmarks --bin profile_browser
  if timeout --signal=TERM --kill-after=5s 30s env VOIDCRAWL_BENCH_NO_SANDBOX=1 valgrind --tool=massif --trace-children=yes --stacks=yes --massif-out-file="$stage/massif.%p.out" "$stage/target/release/profile_browser" navigation_response >"$stage/output.txt" 2>&1; then printf 'scope=fresh controller plus traced Chrome children; Chromium sandbox disabled only inside Valgrind; Massif heap+stack perturbation makes this diagnostic, not a latency comparison.\n' >> "$stage/metadata.txt"; else printf 'massif=unavailable_or_timed_out_chromium_under_valgrind_3.25.1\nscope=no heap result claimed; captured failure retained in output.txt\n' >> "$stage/metadata.txt"; fi ;;
esac
# Build products are not benchmark evidence and can be reconstructed from the
# recorded source/toolchain. Do not duplicate a full Cargo target in each run.
rm -rf "$stage/target"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
run_dir="$class_root/runs/$run_id"
mv "$stage" "$run_dir"
ln -s "runs/$run_id" "$class_root/.latest.$run_id"
mv -Tf "$class_root/.latest.$run_id" "$class_root/latest"
published=true
trap - EXIT
scripts/summarize-benchmark-change.py "$change_dir"
