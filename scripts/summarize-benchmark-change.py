#!/usr/bin/env python3
"""Normalize measured benchmark artifacts; never infer values from artifact presence."""

import csv
import json
import math
import re
import sys
from pathlib import Path

change = Path(sys.argv[1])
rows = [("class", "workload", "metric", "value", "unit", "source", "status")]


def numeric(value):
    try:
        return float(str(value).replace(",", ""))
    except ValueError:
        return None


def add(kind, workload, metric, value, unit, source, status):
    rows.append((kind, workload, metric, value, unit, source, status))


# A class is published through its atomic latest symlink. Legacy flat class
# directories remain readable if no latest pointer exists.
for class_dir in sorted(
    p for p in change.iterdir() if p.is_dir() and not p.name.startswith(".")
):
    kind = class_dir.name
    directory = class_dir / "latest" if (class_dir / "latest").exists() else class_dir
    for estimate in directory.glob("criterion/**/new/estimates.json"):
        try:
            workload = str(estimate.parent.parent.relative_to(directory / "criterion"))
            mean = json.loads(estimate.read_text())["mean"]["point_estimate"]
            source = str(estimate.relative_to(change))
            add(kind, workload, "criterion_mean", mean, "ns", source, "measured")
            sample = json.loads((estimate.parent / "sample.json").read_text())
            durations = sorted(
                time / iterations
                for time, iterations in zip(
                    sample["times"], sample["iters"], strict=True
                )
            )
            for percentile in (50, 95, 99):
                index = max(0, math.ceil(percentile / 100 * len(durations)) - 1)
                add(
                    kind,
                    workload,
                    f"sample_nearest_rank_p{percentile}",
                    durations[index],
                    "ns",
                    source,
                    "measured",
                )
        except (json.JSONDecodeError, KeyError, TypeError, ValueError):  # noqa: PERF203
            add(kind, "", "criterion_mean", "", "ns", estimate.name, "invalid")
    for record in directory.glob("*.jsonl"):
        for line in record.read_text().splitlines():
            try:
                sample = json.loads(line)
                if "aggregate" in sample:
                    workload, aggregate, cleanup = (
                        sample["workload"],
                        sample["aggregate"],
                        sample["cleanup"],
                    )
                    for key, unit in [
                        ("peak_rss_kib", "KiB"),
                        ("peak_pss_kib", "KiB"),
                        ("cpu_ticks_sum", "ticks"),
                        ("fds_peak", "count"),
                        ("tasks_peak", "count"),
                    ]:
                        add(
                            kind,
                            workload,
                            key,
                            aggregate[key],
                            unit,
                            record.name,
                            "measured",
                        )
                    for key in [
                        "close_ms",
                        "process_cleanup_ms",
                        "chrome_count_before_close",
                        "chrome_count_after_close",
                    ]:
                        add(
                            kind,
                            workload,
                            key,
                            cleanup[key],
                            "ms" if key.endswith("_ms") else "count",
                            record.name,
                            "measured",
                        )
                    add(
                        kind,
                        workload,
                        "cleanup_timed_out",
                        cleanup["timed_out"],
                        "boolean",
                        record.name,
                        "measured",
                    )
                elif "completed" in sample:
                    workload = sample["workload"]
                    for key, unit in [
                        ("completed", "count"),
                        ("failures", "count"),
                        ("semaphore_wait_ms_sum", "ms"),
                        ("elapsed_ns", "ns"),
                        ("throughput_per_second", "operations/s"),
                    ]:
                        add(
                            kind,
                            workload,
                            key,
                            sample[key],
                            unit,
                            record.name,
                            "measured",
                        )
            except (json.JSONDecodeError, KeyError, TypeError):  # noqa: PERF203
                add(kind, "", "process", "", "", record.name, "invalid")
    callgrind = directory / "output.txt"
    if kind == "callgrind" and callgrind.exists():
        for line in callgrind.read_text().splitlines():
            match = re.match(
                r"\s*(Instructions|L1 Hits|LL Hits|RAM Hits|Total read\+write|"
                r"Estimated Cycles):\s*([0-9]+)\|",
                line,
            )
            if match:
                add(
                    kind,
                    "fixture_token_count",
                    "callgrind_"
                    + re.sub(r"[^a-z0-9]+", "_", match.group(1).lower()).strip("_"),
                    match.group(2),
                    "count",
                    callgrind.name,
                    "measured",
                )
    allocation = directory / "output.txt"
    if kind == "allocations" and allocation.exists():
        benchmark = None
        section = None
        for line in allocation.read_text().splitlines():
            match = re.search(r"[├╰]─\s+(\S+)", line)
            if match:
                benchmark = match.group(1)
                section = None
            if "max alloc:" in line:
                section = "max_live"
                continue
            if re.search(r"\balloc:\s*│", line) and "max alloc:" not in line:
                section = "allocated"
                continue
            value = re.match(r"\s*│?\s*([0-9][0-9.]*)\s*(B|KB|MB)?\s*│", line)
            if benchmark and section and value:
                metric = (
                    f"{benchmark}_{section}_{'bytes' if value.group(2) else 'count'}"
                )
                multiplier = {None: 1, "B": 1, "KB": 1000, "MB": 1000000}[
                    value.group(2)
                ]
                add(
                    kind,
                    benchmark,
                    metric,
                    float(value.group(1)) * multiplier,
                    "bytes" if value.group(2) else "count",
                    allocation.name,
                    "measured",
                )
                if value.group(2):
                    section = None
    metadata = (
        (directory / "metadata.txt").read_text()
        if (directory / "metadata.txt").exists()
        else ""
    )
    if kind == "heap":
        if "massif=unavailable_" in metadata:
            add(
                kind,
                "navigation_response",
                "massif",
                "unavailable",
                "",
                "metadata.txt",
                "unavailable",
            )
        else:
            for massif in directory.glob("massif.*.out"):
                peak = 0
                current = {}
                for line in massif.read_text(errors="replace").splitlines():
                    if "=" in line:
                        key, value = line.split("=", 1)
                        if (
                            key in {"mem_heap_B", "mem_heap_extra_B", "mem_stacks_B"}
                            and value.isdigit()
                        ):
                            current[key] = int(value)
                            if len(current) == 3:
                                peak = max(peak, sum(current.values()))
                                current = {}
                add(
                    kind,
                    "navigation_response",
                    "massif_peak_heap_extra_stack_bytes",
                    peak,
                    "bytes",
                    massif.name,
                    "measured" if peak else "invalid",
                )
    perf = directory / "perf.csv"
    if perf.exists():
        valid = False
        for line in perf.read_text().splitlines():
            parts = line.split(",")
            event = parts[2].split(":", 1)[0] if len(parts) >= 3 else ""
            if event in {"cycles", "instructions"}:
                value = numeric(parts[0])
                add(
                    kind,
                    "navigation_response",
                    f"perf_{event}",
                    parts[0],
                    "count",
                    perf.name,
                    "measured" if value is not None else "invalid",
                )
                valid |= value is not None
        if not valid:
            add(kind, "", "perf", "", "", perf.name, "invalid")
    elif "unavailable_or_permission_denied" in metadata:
        add(kind, "", "perf", "unavailable", "", "metadata.txt", "unavailable")
    elif kind == "process":
        add(kind, "", "perf", "", "", "", "missing")
with (change / "summary.csv").open("w", newline="") as f:
    csv.writer(f).writerows(rows)
with (change / "README.md").open("w") as f:
    f.write(
        "# VoidCrawl benchmark change\n\n"
        "Only rows marked `measured` are numeric results. "
        "`sample_nearest_rank_p*` values are harness-calculated nearest-rank "
        "sample values, not Criterion-native or release-grade percentiles. "
        "Raw chromiumoxide/CDP comparison is unavailable: no equivalent "
        "supported raw wrapper path exists without duplicating private setup.\n\n"
    )
    f.write(
        "| class | workload | metric | value | unit | source | status |\n"
        "|---|---|---|---:|---|---|---|\n"
    )
    for row in rows[1:]:
        f.write(f"| {' | '.join(map(str, row))} |\n")
