"""Manual challenge operator loop for real VoidCrawl sessions.

This is intentionally boring QA glue:

1. Open a real headful browser, either local Chrome or Docker headful Chrome.
2. Navigate to a real URL.
3. Print same-tab attach coordinates and VNC/noVNC links.
4. Print the anti-bot verdict and DOM captcha kind.
5. Wait for you to clear the page manually.
6. Re-probe and print whether the wall is still visible.

Run local headful Chrome:

    uv run python examples/manual_challenge_operator.py \
      --url https://2captcha.com/demo/cloudflare-turnstile

Run Docker headful Chrome with noVNC:

    ./docker/run-headful.sh
    uv run python examples/manual_challenge_operator.py \
      --docker-headful \
      --url https://2captcha.com/demo/cloudflare-turnstile

Open noVNC in your browser:

    http://127.0.0.1:6080
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import urllib.request
from pathlib import Path
from typing import Any

from voidcrawl import BrowserConfig, BrowserSession

DEFAULT_URL = "https://2captcha.com/demo/cloudflare-turnstile"
DEFAULT_NOVNC_URL = "http://127.0.0.1:6080"
DEFAULT_VNC_URL = "vnc://127.0.0.1:5900"
DEFAULT_DOCKER_CDP_VERSION_URL = "http://127.0.0.1:19222/json/version"


def resolve_docker_ws_url(version_url: str) -> str:
    with urllib.request.urlopen(version_url, timeout=3) as response:
        payload = json.loads(response.read().decode("utf-8"))
    ws_url = payload.get("webSocketDebuggerUrl")
    if not isinstance(ws_url, str) or not ws_url:
        raise RuntimeError(f"no webSocketDebuggerUrl in {version_url}")
    return ws_url


def antibot_to_dict(antibot: Any) -> dict[str, Any] | None:
    if antibot is None:
        return None
    return {
        "vendors": list(getattr(antibot, "vendors", []) or []),
        "challenged": bool(getattr(antibot, "challenged", False)),
        "challenge_vendor": getattr(antibot, "challenge_vendor", None),
        "corpus_version": getattr(antibot, "corpus_version", None),
        "evidence": getattr(antibot, "evidence", None),
    }


async def prompt(message: str) -> str:
    return await asyncio.to_thread(input, message)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--port", type=int, default=9222)
    parser.add_argument("--docker-headful", action="store_true")
    parser.add_argument("--docker-version-url", default=DEFAULT_DOCKER_CDP_VERSION_URL)
    parser.add_argument(
        "--novnc-url", default=os.environ.get("VOIDCRAWL_NOVNC_URL", DEFAULT_NOVNC_URL)
    )
    parser.add_argument(
        "--vnc-url", default=os.environ.get("VOIDCRAWL_VNC_URL", DEFAULT_VNC_URL)
    )
    parser.add_argument("--screenshot-dir", default="/tmp/voidcrawl-challenge-operator")
    return parser.parse_args()


def browser_config(args: argparse.Namespace) -> tuple[BrowserConfig, str]:
    if args.docker_headful:
        ws_url = resolve_docker_ws_url(args.docker_version_url)
        return (
            BrowserConfig(ws_url=ws_url, headless=False),
            f"Docker headful Chrome via {args.docker_version_url}",
        )
    return (
        BrowserConfig(headless=False, port=args.port),
        f"local headful Chrome on CDP port {args.port}",
    )


async def main() -> None:
    args = parse_args()
    screenshot_dir = Path(args.screenshot_dir)
    screenshot_dir.mkdir(parents=True, exist_ok=True)
    config, browser_label = browser_config(args)

    print(f"browser: {browser_label}")
    print(f"url:     {args.url}")
    print(f"noVNC:   {args.novnc_url}")
    print(f"VNC:     {args.vnc_url}")

    async with BrowserSession(config) as browser:
        page = await browser.new_page("about:blank")
        response = await page.goto(args.url, timeout=args.timeout)
        title = await page.title()
        final_url = await page.url()
        target_id = await page.target_id()
        websocket_url = await browser.websocket_url()
        captcha_before = await page.detect_captcha()
        ax_outline = await page.ax_tree_outline(3)
        before_png = screenshot_dir / "before.png"
        before_png.write_bytes(await page.screenshot_png())

        print("\nattach coordinates")
        print(
            json.dumps(
                {
                    "websocket_url": websocket_url,
                    "target_id": target_id,
                    "novnc_url": args.novnc_url,
                    "vnc_url": args.vnc_url,
                },
                indent=2,
            )
        )

        print("\ninitial probe")
        print(
            json.dumps(
                {
                    "title": title,
                    "url": final_url,
                    "status_code": getattr(response, "status_code", None),
                    "redirected": getattr(response, "redirected", None),
                    "antibot": antibot_to_dict(getattr(response, "antibot", None)),
                    "dom_captcha": captcha_before,
                    "screenshot": str(before_png),
                },
                indent=2,
            )
        )

        print("\nax outline")
        print((ax_outline or "").strip()[:4000])

        if captcha_before:
            print(
                "\nOpen the browser/noVNC and clear "
                f"`{captcha_before}` in this same tab."
            )
        else:
            print(
                "\nNo DOM captcha was detected. "
                "You can still inspect the visible browser."
            )

        await prompt(
            "Press Enter after the page is cleared or you decide it is not clearable..."
        )

        try:
            await page.wait_for_network_idle(timeout=5.0)
        except Exception as exc:
            print(f"network idle wait did not complete cleanly: {exc}")

        captcha_after = await page.detect_captcha()
        after_title = await page.title()
        after_url = await page.url()
        html = await page.content()
        after_png = screenshot_dir / "after.png"
        after_png.write_bytes(await page.screenshot_png())

        print("\nafter manual action")
        print(
            json.dumps(
                {
                    "title": after_title,
                    "url": after_url,
                    "dom_captcha": captcha_after,
                    "captcha_cleared": captcha_before is not None
                    and captcha_after is None,
                    "html_len": len(html),
                    "screenshot": str(after_png),
                },
                indent=2,
            )
        )

        if captcha_after:
            raise SystemExit(f"still blocked by captcha kind: {captcha_after}")


if __name__ == "__main__":
    asyncio.run(main())
