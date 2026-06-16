"""Reach Cloudflare Turnstile's checkbox — closed shadow root, cross-origin frame.

Turnstile's "Verify you are human" checkbox is a real `<input type=checkbox>`,
but it lives in a **closed** shadow root inside a **cross-origin**
`challenges.cloudflare.com` iframe. That puts it out of reach of every page-JS
trick: `element.shadowRoot` is `null` for closed roots, and `eval_js_in_frame`
runs in page-world JS so it cannot pierce them either. Force-opening the shadow
DOM is not an option — VoidCrawl removed that patch because Turnstile
tamper-checks its closed root and fails the challenge with `ERROR 600010`.

The accessibility tree is the way in. `Accessibility.getFullAXTree` is
browser-computed and **ignores shadow-DOM mode**, so rooting it at the Turnstile
frame descends straight into the closed root. `ax_outline_in_frame` surfaces the
`checkbox "Verify you are human"`; `ax_box_in_frame` returns its on-page rect;
and a **trusted compositor** click (`dispatch_mouse_event`, not a DOM `.click()`,
which the widget rejects as untrusted) mints the `cf-turnstile-response` token —
with NO page-JS shadow tampering.

Like `eval_js_in_frame`, this needs the Turnstile frame in the page's renderer
process. `challenges.cloudflare.com` is field-trial-isolated out-of-process by
default, so launch with `extra_args=["disable-site-isolation-trials"]` or the
frame is unreachable (`FrameNotFound`).

Run:
    uv run python examples/turnstile_checkbox_ax.py

Hits a live third-party demo that embeds managed Turnstile; degrades gracefully
without network or if the demo changes.
"""

import asyncio
import random

from voidcrawl import BrowserConfig, BrowserSession

_FRAME = "challenges.cloudflare.com"
_DEMO = "https://2captcha.com/demo/cloudflare-turnstile"


async def _poll(coro_factory, *, predicate, tries=30, delay=0.5):
    """Retry an async call until `predicate(result)` holds (or give up).

    Turnstile injects its frame and its token asynchronously after load, so
    polling is steadier than a fixed sleep on a slow or loaded machine.
    """
    for _ in range(tries):
        try:
            result = await coro_factory()
            if predicate(result):
                return result
        except Exception:
            pass
        await asyncio.sleep(delay)
    return None


async def _token(page) -> str | None:
    """Read the Turnstile token off the hidden response input, if minted yet."""
    return await page.evaluate_js(
        "(() => { const i ="
        " document.querySelector('input[name=cf-turnstile-response]');"
        " return i && i.value ? i.value : ''; })()"
    )


async def main() -> None:
    print("=== Cloudflare Turnstile checkbox via AX-in-frame (closed shadow) ===")
    # challenges.cloudflare.com is field-trial-isolated out-of-process; opt in
    # to keep its frame reachable. (A pool/session without this flag would get
    # FrameNotFound below.) Headful clears managed Turnstile non-interactively;
    # this demo's checkbox is interactive, so headless is fine to drive.
    browser = BrowserSession(
        BrowserConfig(extra_args=["disable-site-isolation-trials"])
    )

    async with browser:
        try:
            page = await browser.new_page(_DEMO)
        except Exception as exc:
            print(f"  (skipped — could not load demo page: {exc})")
            return

        # 1. DISCOVER — outline the Turnstile frame's AX tree (pierces the closed
        #    shadow root) to read the exact role + accessible name to target.
        outline = await _poll(
            lambda: page.ax_outline_in_frame(_FRAME),
            predicate=lambda v: v and "checkbox" in v,
            tries=20,
        )
        if outline is None:
            print("  (Turnstile frame never appeared — network or demo changed)")
            return
        print("  ax_outline_in_frame (closed shadow root, cross-origin):")
        for line in outline.splitlines():
            if "checkbox" in line or "Verify" in line:
                print(f"    {line.strip()}")

        # 2. LOCATE — the checkbox rect [x, y, width, height] in CSS px. Empty
        #    name would match any checkbox; here we name it explicitly.
        box = await page.ax_box_in_frame(_FRAME, "checkbox", "Verify you are human")
        x, y, w, h = box
        rect = [round(v, 1) for v in box]
        print(f"  ax_box_in_frame -> rect {rect}  ({w:.0f}x{h:.0f})")

        # 3. CLICK — a humanized compositor click at a jittered point in the box.
        #    Trusted input (isTrusted=true); a DOM .click() would be rejected.
        #    `click_ax_in_frame(...)` does the centre click in one call; we drive
        #    it by hand here to show the jitter the box geometry buys you.
        px = x + w / 2 + random.uniform(-w / 6, w / 6)
        py = y + h / 2 + random.uniform(-h / 6, h / 6)
        await page.dispatch_mouse_event("mouseMoved", px, py)
        await page.dispatch_mouse_event("mousePressed", px, py)
        await page.dispatch_mouse_event("mouseReleased", px, py)
        print(f"  compositor click at ({px:.0f}, {py:.0f})")

        # 4. CONFIRM — the token is minted into the hidden response input.
        token = await _poll(_token, predicate=bool, tries=30)
        if token:
            n = len(token)
            print(f"  cf-turnstile-response minted -> {token[:24]} ({n} chars)")
        else:
            print(
                "  (no token yet — the demo may have served an interactive challenge)"
            )


if __name__ == "__main__":
    asyncio.run(main())
