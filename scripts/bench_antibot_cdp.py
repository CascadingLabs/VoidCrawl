#!/usr/bin/env python3
"""Compare VoidCrawl against nodriver on anti-bot pages.

This is an operator benchmark: pass real target URLs explicitly. It records
whether a page appears passed, challenged, blocked, or errored without embedding
third-party test targets in the repository.

Examples:
    uv run python scripts/bench_antibot_cdp.py \
      --url https://example-cloudflare-managed-challenge.test \
      --url https://example-datadome-style.test \
      --runs 3 --headful --engine voidcrawl --engine nodriver
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import importlib
import json
import time
from dataclasses import asdict, dataclass
from typing import Any, Literal, cast

from voidcrawl import BrowserConfig, BrowserSession

Engine = Literal["voidcrawl", "nodriver"]
Verdict = Literal["passed", "challenged", "blocked", "error"]

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
) -> Result:
    start = time.perf_counter()
    try:
        async with BrowserSession(
            BrowserConfig(headless=not headful, ws_url=ws_url)
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
        )
    except Exception as exc:
        return Result(engine, url, run, "error", elapsed_ms(start), error=repr(exc))


async def run_nodriver(
    url: str,
    run: int,
    headful: bool,
    timeout: float,
    settle_secs: float,
) -> Result:
    start = time.perf_counter()
    browser: Any | None = None
    try:
        uc = importlib.import_module("nodriver")
        browser = await asyncio.wait_for(
            uc.start(headless=not headful), timeout=timeout
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
            "nodriver",
            url,
            run,
            classify(title or "", html or ""),
            elapsed_ms(start),
            title,
            fingerprint=fingerprint,
        )
    except Exception as exc:
        return Result("nodriver", url, run, "error", elapsed_ms(start), error=repr(exc))
    finally:
        if browser is not None:
            with contextlib.suppress(Exception):
                browser.stop()


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
        choices=["voidcrawl", "nodriver"],
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
    args = parser.parse_args()

    engines: list[Engine] = args.engine or ["voidcrawl", "nodriver"]
    results: list[Result] = []
    for url in args.url:
        for run in range(1, args.runs + 1):
            for engine in engines:
                if engine == "nodriver":
                    result = await run_nodriver(
                        url, run, args.headful, args.timeout, args.settle_secs
                    )
                else:
                    result = await run_voidcrawl(
                        engine,
                        url,
                        run,
                        args.headful,
                        args.timeout,
                        args.settle_secs,
                        args.voidcrawl_ws_url,
                    )
                results.append(result)
                print(json.dumps(asdict(result), sort_keys=True), flush=True)

    summary: dict[str, dict[str, int]] = {}
    for result in results:
        key = f"{result.engine} {result.url}"
        bucket = summary.setdefault(key, {})
        bucket[result.verdict] = bucket.get(result.verdict, 0) + 1
    print(json.dumps({"summary": summary}, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(main())
