---
name: voidcrawl-benchmarking
description: Measure VoidCrawl L2 browser/CDP workloads safely, reproducibly, and with evidence that supports comparison.
---
# VoidCrawl L2 benchmarking

## Before measurement
- Use only committed fixtures through the ephemeral loopback HTTP server. Reject `data:`, `file:`, public URLs, and fixtures whose manifest/generator check fails.
- Run `python3 scripts/generate-benchmark-fixtures.py --check` and `cargo xtask benchmark check` before any measurement. Do not optimize, establish thresholds, or claim regressions from one local run.
- Run benchmark classes sequentially. Criterion, Callgrind, Divan, Massif, process sampling, and perf answer different questions and must never be combined into a score.
- Launch/warm Chrome and warm named pools outside every warm iteration. A timed launch is valid only in a workload explicitly named `cold_*`.

## Named metric semantics
- **Criterion mean** is controller wall-clock time for exactly the named operation; it is not browser CPU time.
- **Throughput** is completed matrix operations / elapsed wall time. Report failure count and summed semaphore-acquire wait alongside it; do not hide timed-out acquires.
- **RSS/PSS peak**, FD/task peaks, CPU-tick sum, and role-count peaks are repeated `/proc` tree samples over the workload, not controller-only snapshots.
- Chrome roles must come from `/proc/<pid>/cmdline`: `browser`, `renderer`, `gpu-process`, or `utility`; unknown/controller processes remain explicit.
- **Cleanup** is close elapsed time plus Chrome descendant counts before/after close. `perf` is numeric only when parsable; permission/tool absence is `unavailable`, never zero.
- Callgrind/Divan fixture-only results do not claim CDP/browser coverage. Massif perturbs execution and is diagnostic, not latency evidence.

## Evidence and publication
- Result classes publish atomically to `voidcrawl-benchmarks/results/by-change/jj/<stable-change-id>/<class>` (git fallback). Capture the source snapshot before writes, then regenerate README and normalized CSV.
- Dashboard rows must contain measured numeric data and source path. Missing, invalid, and unavailable data are distinct statuses; artifact presence alone is not a metric.
- Preserve fixture route, size, digest, content type, and generator metadata. Comparisons require matching fixture digest, browser/runtime metadata, workload name, and metric semantics.
- A raw chromiumoxide/CDP comparison may be reported only if it uses an equivalent public setup and operation. Otherwise encode and document `unavailable` with the concrete reason; never use a placeholder baseline.

## JJ comparison workflow
1. Capture `jj log -r @ --no-graph -T 'change_id ++ "\n"'` before result writes (the generated output can alter the working change).
2. Run one class at a time with `cargo xtask benchmark <class>`; retain each class metadata and raw artifacts.
3. Compare only two completed directories with the same manifest digest and named workload. Use normalized CSV rows, then inspect raw Criterion/process/perf evidence for any surprising delta.
4. State the two change IDs, command, date/environment, fixture digest, and unavailable collection in the review note. A local result is evidence, not a release gate.
