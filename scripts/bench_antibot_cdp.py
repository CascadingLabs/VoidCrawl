#!/usr/bin/env python3
"""Compare VoidCrawl against nodriver on anti-bot pages.

This is an operator benchmark: pass real target URLs explicitly. It records
whether a page appears passed, challenged, blocked, or errored without embedding
third-party test targets in the repository.

Examples:
    uv run python scripts/bench_antibot_cdp.py \
      --url https://example-cloudflare-managed-challenge.test \
      --url https://example-datadome-style.test \
      --runs 3 --headful --engine voidcrawl --engine nodriver --engine zendriver

    # The comparison that matters for CAS-217: VoidCrawl's default CDP surface
    # vs. its minimal one, against the same target.
    uv run python scripts/bench_antibot_cdp.py \
      --url https://example-cloudflare-managed-challenge.test \
      --engine voidcrawl --cdp-mode normal --cdp-mode minimal --runs 3
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import importlib
import inspect
import json
import time
from dataclasses import asdict, dataclass
from typing import Any, Literal, cast

from voidcrawl import BrowserConfig, BrowserSession

Engine = Literal["voidcrawl", "nodriver", "zendriver"]
Verdict = Literal["passed", "challenged", "blocked", "error"]
CdpMode = Literal["normal", "minimal"]

CHALLENGE_MARKERS = (
    "just a moment",
    "checking your browser",
    "verify you are human",
    "cf-challenge",
    "datadome",
)
BLOCK_MARKERS = ("access denied", "forbidden", "error 1020", "403 forbidden")

FINGERPRINT_VALUE_JS = r"""
(() => {
  const webgl = (() => {
    try {
      const canvas = document.createElement('canvas');
      const gl = canvas.getContext('webgl') || canvas.getContext('experimental-webgl');
      if (!gl) return {renderer: null, vendor: null};
      const ext = gl.getExtension('WEBGL_debug_renderer_info');
      return ext ? {
        renderer: gl.getParameter(ext.UNMASKED_RENDERER_WEBGL),
        vendor: gl.getParameter(ext.UNMASKED_VENDOR_WEBGL),
      } : {renderer: null, vendor: null};
    } catch (err) {
      return {error: String(err)};
    }
  })();
  return {
    webdriver: navigator.webdriver,
    userAgent: navigator.userAgent,
    platform: navigator.platform,
    languages: Array.from(navigator.languages || []),
    hardwareConcurrency: navigator.hardwareConcurrency,
    deviceMemory: navigator.deviceMemory || null,
    screen: {
      width: screen.width,
      height: screen.height,
      availWidth: screen.availWidth,
      availHeight: screen.availHeight,
      colorDepth: screen.colorDepth,
      pixelDepth: screen.pixelDepth,
    },
    viewport: {width: innerWidth, height: innerHeight, dpr: devicePixelRatio},
    userAgentData: navigator.userAgentData ? {
      brands: navigator.userAgentData.brands,
      mobile: navigator.userAgentData.mobile,
      platform: navigator.userAgentData.platform,
    } : null,
    webgl,
  };
})()
"""
FINGERPRINT_JSON_JS = f"JSON.stringify({FINGERPRINT_VALUE_JS})"


@dataclass
class Result:
    engine: str
    url: str
    run: int
    verdict: Verdict
    elapsed_ms: int
    title: str | None = None
    error: str | None = None
    fingerprint: dict[str, Any] | None = None
    #: Only set for the ``voidcrawl`` engine — the other drivers have no
    #: equivalent knob. Part of the record so a run comparing modes stays
    #: interpretable after the fact.
    cdp_mode: CdpMode | None = None


def elapsed_ms(start: float) -> int:
    return int((time.perf_counter() - start) * 1000)


def as_fingerprint(value: object) -> dict[str, Any] | None:
    if isinstance(value, dict):
        return value
    if isinstance(value, str):
        try:
            parsed = json.loads(value)
        except json.JSONDecodeError:
            return None
        return parsed if isinstance(parsed, dict) else None
    return None


def classify(title: str, html: str) -> Verdict:
    title_text = title.lower().strip()
    body_text = html.lower()
    text = f"{title_text}\n{body_text}"
    if any(marker in text for marker in BLOCK_MARKERS):
        return "blocked"
    if any(marker in text for marker in CHALLENGE_MARKERS):
        return "challenged"
    return "passed"


async def run_voidcrawl(
    engine: Engine,
    url: str,
    run: int,
    headful: bool,
    timeout: float,
    settle_secs: float,
    ws_url: str | None,
    cdp_mode: CdpMode | None = None,
) -> Result:
    start = time.perf_counter()

    async def run_once() -> Result:
        async with BrowserSession(
            BrowserConfig(headless=not headful, ws_url=ws_url, cdp_mode=cdp_mode)
        ) as browser:
            page = await asyncio.wait_for(browser.new_page(url), timeout=timeout)
            await asyncio.sleep(settle_secs)
            title = await asyncio.wait_for(page.title(), timeout=timeout)
            fingerprint = as_fingerprint(
                await asyncio.wait_for(
                    page.eval_js(FINGERPRINT_JSON_JS), timeout=timeout
                )
            )
            html = await asyncio.wait_for(page.content(), timeout=timeout)
        return Result(
            engine,
            url,
            run,
            classify(title, html),
            elapsed_ms(start),
            title,
            fingerprint=fingerprint,
            cdp_mode=cdp_mode,
        )

    try:
        return await asyncio.wait_for(run_once(), timeout=timeout + settle_secs + 5.0)
    except Exception as exc:
        return Result(
            engine,
            url,
            run,
            "error",
            elapsed_ms(start),
            error=repr(exc),
            cdp_mode=cdp_mode,
        )


async def stop_browser(browser: Any) -> None:
    with contextlib.suppress(Exception):
        result = browser.stop()
        if inspect.isawaitable(result):
            await result


async def run_driver_like(
    engine: Literal["nodriver", "zendriver"],
    url: str,
    run: int,
    headful: bool,
    timeout: float,
    settle_secs: float,
) -> Result:
    start = time.perf_counter()
    browser: Any | None = None
    try:
        driver = importlib.import_module(engine)
        browser = await asyncio.wait_for(
            driver.start(headless=not headful), timeout=timeout
        )
        page = await asyncio.wait_for(browser.get(url), timeout=timeout)
        await asyncio.sleep(settle_secs)
        title = cast(
            "str | None",
            await asyncio.wait_for(page.evaluate("document.title"), timeout=timeout),
        )
        fingerprint = as_fingerprint(
            await asyncio.wait_for(page.evaluate(FINGERPRINT_JSON_JS), timeout=timeout)
        )
        html = cast(
            "str | None", await asyncio.wait_for(page.get_content(), timeout=timeout)
        )
        return Result(
            engine,
            url,
            run,
            classify(title or "", html or ""),
            elapsed_ms(start),
            title,
            fingerprint=fingerprint,
        )
    except Exception as exc:
        return Result(engine, url, run, "error", elapsed_ms(start), error=repr(exc))
    finally:
        if browser is not None:
            await stop_browser(browser)


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--url",
        action="append",
        required=True,
        help="Cloudflare/DataDome-style target URL; repeatable",
    )
    parser.add_argument(
        "--engine",
        action="append",
        choices=["voidcrawl", "nodriver", "zendriver"],
        default=[],
    )
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument(
        "--settle-secs",
        type=float,
        default=8.0,
        help="Seconds to wait before classifying challenge state",
    )
    parser.add_argument(
        "--headful", action="store_true", help="Run visible/headful browsers"
    )
    parser.add_argument(
        "--voidcrawl-ws-url",
        help=("Connect VoidCrawl to an existing Docker/remote Chrome CDP endpoint"),
    )
    parser.add_argument(
        "--cdp-mode",
        action="append",
        choices=["normal", "minimal"],
        default=[],
        help=(
            "VoidCrawl CDP surface to test; repeatable to compare modes against "
            "the same target. Ignored for nodriver/zendriver. Defaults to the "
            "library default (normal)."
        ),
    )
    args = parser.parse_args()

    engines: list[Engine] = args.engine or ["voidcrawl", "nodriver", "zendriver"]
    # `[None]` means "don't pass cdp_mode at all", which exercises the library
    # default rather than pinning it — a distinct case worth being able to test.
    cdp_modes: list[CdpMode | None] = list(args.cdp_mode) or [None]
    results: list[Result] = []
    for url in args.url:
        for run in range(1, args.runs + 1):
            for engine in engines:
                if engine in {"nodriver", "zendriver"}:
                    results.append(
                        await run_driver_like(
                            engine,
                            url,
                            run,
                            args.headful,
                            args.timeout,
                            args.settle_secs,
                        )
                    )
                    print(json.dumps(asdict(results[-1]), sort_keys=True), flush=True)
                    continue
                for cdp_mode in cdp_modes:
                    results.append(
                        await run_voidcrawl(
                            engine,
                            url,
                            run,
                            args.headful,
                            args.timeout,
                            args.settle_secs,
                            args.voidcrawl_ws_url,
                            cdp_mode,
                        )
                    )
                    print(json.dumps(asdict(results[-1]), sort_keys=True), flush=True)

    summary: dict[str, dict[str, int]] = {}
    for result in results:
        # Keep cdp_mode in the key — collapsing normal and minimal into one
        # bucket would hide the exact difference this benchmark exists to show.
        label = (
            f"{result.engine}[{result.cdp_mode}]" if result.cdp_mode else result.engine
        )
        key = f"{label} {result.url}"
        bucket = summary.setdefault(key, {})
        bucket[result.verdict] = bucket.get(result.verdict, 0) + 1
    print(json.dumps({"summary": summary}, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(main())
