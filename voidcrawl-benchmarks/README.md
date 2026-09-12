# VoidCrawl L2 browser benchmarks

Inputs are committed fixtures served only by an ephemeral `127.0.0.1` HTTP server. `fixtures/manifest.json` records route, size, digest, content type, and generator; validate it with `python3 scripts/generate-benchmark-fixtures.py --check`.

`cargo xtask benchmark check` compiles every harness. Warm Criterion operations reuse pre-launched Chrome; `warm_network_byte_control` arms typed `NavigationCapture` and consumes its byte report. The warm concurrency matrix is 1×1×1, 1×2×2, and 2×2×4, reporting completed throughput, summed semaphore wait, and failures through the workload result.

## CAS-322 byte-control handoff

The existing `warm_network_byte_control` case is a complete-path baseline, not byte-pressure evidence. The final CAS-317 comparison must measure unlike cases separately:

- exact limit and one decoded byte over the limit;
- materially truncated CDP decoded bodies, including base64 expansion;
- per-response truncation versus exhaustion of a shared response-capture budget;
- runtime diagnostic truncation that backs off at a multibyte UTF-8 boundary;
- AX node pressure, ordinary byte pressure, and the minimum valid JSON bound;
- retained frame totals across multiple recording regions and encoded-output size accounting;
- cancellation/deadline cleanup after partial byte admission.

Record provider-materialization separately from retained allocation/copy cost. DOM, AX, screenshots, response-body strings, and recording frames can already exist in Chromium or the CDP client before VoidCrawl applies a retention bound; none of these cases establishes a peak-browser-memory limit.

Process runs sample the controller/Chrome tree concurrently at a bounded interval, aggregate peaks/sums and cmdline-derived Chrome roles, and record post-close timing/counts. Perf is numeric only when parseable and otherwise explicitly unavailable. Callgrind and Divan are fixture-only; Massif is diagnostic. A raw chromiumoxide comparison is explicitly unavailable because no equivalent supported public lifecycle/byte-control setup exists without duplicating private setup.

Results are local under `voidcrawl-benchmarks/results/by-change/{jj|git}/…`; compare sequential classes only when fixture and environment metadata match.
