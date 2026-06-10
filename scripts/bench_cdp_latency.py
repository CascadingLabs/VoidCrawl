"""CDP per-op latency benchmark — container vs local (CAS-210).

Measures the primitive CDP ops the solvers lean on (eval_js, screenshot,
click) against one or more attach targets, plus throttling diagnostics
that distinguish "transport is slow" from "renderer is backgrounded".

Usage:
    uv run python scripts/bench_cdp_latency.py \
        --target chrome-1=http://localhost:19222 \
        --target chrome-2=http://localhost:19223 \
        --local
"""

import argparse
import asyncio
import statistics
import time

from voidcrawl import BrowserConfig, BrowserSession

PAGE_HTML = (
    "<title>bench</title><body>"
    + "".join(f"<div class='c'>cell {i}</div>" for i in range(400))
    + "<button id='btn' onclick='window.__clicks=(window.__clicks||0)+1'>go</button>"
    "</body>"
)
PAGE_URL = "data:text/html," + PAGE_HTML.replace("#", "%23")


async def timed(coro_factory, n: int) -> dict[str, float]:
    """Run *coro_factory()* n times, return latency stats in ms."""
    samples: list[float] = []
    for _ in range(n):
        t0 = time.perf_counter()
        await coro_factory()
        samples.append((time.perf_counter() - t0) * 1000)
    # Small N by design (these ops are slow enough that 8 samples is plenty
    # to separate "33ms" from "stalled"). Report median and max only — a
    # percentile off <100 samples would be noise dressed as rigor.
    return {
        "median": statistics.median(samples),
        "max": max(samples),
    }


async def throttle_probes(page) -> dict[str, object]:
    """Detect renderer backgrounding: visibility, timer clamp, frame output."""
    visibility = await page.evaluate_js("document.visibilityState")
    await page.evaluate_js(
        "window.__dt=-1; window.__raf=-1;"
        "window.__t0=performance.now();"
        "setTimeout(()=>{window.__dt=performance.now()-window.__t0},0);"
        "requestAnimationFrame(()=>{window.__raf=performance.now()-window.__t0});"
        "0"
    )
    await asyncio.sleep(2.0)
    timer_ms = await page.evaluate_js("window.__dt")
    raf_ms = await page.evaluate_js("window.__raf")
    return {
        "visibility": visibility,
        "setTimeout(0) fired after (ms)": timer_ms,
        "rAF fired after (ms, -1 = never)": raf_ms,
    }


async def bench_target(label: str, ws_url: str | None) -> None:
    print(f"\n=== {label} ({ws_url or 'locally launched'}) ===")
    config = (
        BrowserConfig(ws_url=ws_url)
        if ws_url
        else BrowserConfig(headless=True, no_sandbox=True)
    )
    session = BrowserSession(config)
    async with session:
        page = await session.new_page(PAGE_URL)
        try:
            for k, v in (await throttle_probes(page)).items():
                print(f"  [probe] {k}: {v}")

            ops = {
                "eval (1+1)": lambda: page.evaluate_js("1+1"),
                "eval (DOM count)": lambda: page.evaluate_js(
                    "document.querySelectorAll('div.c').length"
                ),
                "click #btn": lambda: page.click_element("#btn"),
                "screenshot bbox 300x300": lambda: page.screenshot(
                    bbox=(0, 0, 300, 300)
                ),
                "screenshot full": page.screenshot_png,
            }
            for name, factory in ops.items():
                n = 3 if "screenshot" in name else 8
                stats = await timed(factory, n)
                print(
                    f"  {name:28s} median {stats['median']:8.1f} ms   "
                    f"max {stats['max']:8.1f} ms   (n={n})"
                )
        finally:
            await page.close()


async def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target",
        action="append",
        default=[],
        metavar="LABEL=WS_URL",
        help="attach target, e.g. chrome-1=http://localhost:19222",
    )
    parser.add_argument(
        "--local",
        action="store_true",
        help="also benchmark a locally launched headless browser as the floor",
    )
    args = parser.parse_args()

    for spec in args.target:
        label, _, ws_url = spec.partition("=")
        await bench_target(label, ws_url)
    if args.local:
        await bench_target("local-headless", None)


if __name__ == "__main__":
    asyncio.run(main())
