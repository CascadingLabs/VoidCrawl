"""Read & drive a CROSS-ORIGIN iframe's DOM — impossible from page JS, easy over CDP.

Same-origin policy makes a cross-origin iframe's `contentDocument` return `null`
to the parent's JavaScript. That is a *page-script* restriction; it does not
apply to the controlling debugger. `evaluate_js_in_frame(url_pattern, expr)`
evaluates `expr` inside the target frame's OWN execution context — where
`document` is that frame's document and the origin check is satisfied. This is
how you reach Stripe/Braintree payment fields, OAuth iframes, consent walls, and
reCAPTCHA's `bframe` on real third-party sites.

The frame must be in the page's renderer process for its context to be reachable.
VoidCrawl's defaults keep ordinary cross-origin frames in-process (Part 1), but
Chrome field-trial-isolates a few origins — notably google.com, so reCAPTCHA —
out-of-process regardless. Reaching THOSE is an explicit opt-in: launch with
`extra_args=["disable-site-isolation-trials"]` (Part 2). It is not a global
default because it weakens the browser's isolation posture.

Run:
    uv run python examples/cross_origin_iframe_eval.py

Part 1 is self-contained (a data: page embedding a separate-origin data: child),
so it always runs. Part 2 hits a live third-party page that embeds reCAPTCHA
cross-origin; it degrades gracefully without network.
"""

import asyncio
import base64

from voidcrawl import BrowserConfig, BrowserPool, BrowserSession, PoolConfig

# ── Part 1 fixture: a parent that embeds a genuinely cross-origin child ──────
# Each data: URL gets its own opaque origin, and opaque origins never match — so
# the child is cross-origin to the parent. The child URL is base64'd and only
# materialized via atob() so the marker "CHILDFRAME" appears ONLY in the child's
# URL (mirrors how a real site URL never contains "api2/bframe").
_CHILD = (
    "data:text/html,<p>CHILDFRAME</p><div id=secret>the-cross-origin-secret-42</div>"
)
_B64 = base64.b64encode(_CHILD.encode()).decode()
_PARENT = (
    "data:text/html,"
    "<h1>parent page</h1><iframe id=f></iframe>"
    f"<script>document.getElementById('f').src=atob('{_B64}')</script>"
)

# Probe the child from the PARENT's context (the same-origin wall).
_PARENT_PROBE = """
(() => {
  const f = document.querySelector('iframe');
  try {
    return f.contentDocument
      ? f.contentDocument.getElementById('secret').textContent
      : 'NULL — contentDocument blocked (cross-origin)';
  } catch (e) {
    return 'THROW — ' + e.name + ' (cross-origin)';
  }
})()
"""


async def _poll(coro_factory, *, predicate, tries=30, delay=0.2):
    """Retry an async call until `predicate(result)` holds (or give up).

    The child iframe is attached by script, so it registers a beat after load;
    polling is steadier than a fixed sleep on a slow/loaded machine.
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


async def part1_self_contained(tab) -> None:
    print("\n=== Part 1: cross-origin data: iframe (self-contained) ===")
    await tab.goto(_PARENT)

    from_parent = await tab.evaluate_js(_PARENT_PROBE)
    print(f"  parent  evaluate_js          → {from_parent!r}")

    # Reach INTO the child's own context. `document` here is the child's.
    secret = await _poll(
        lambda: tab.evaluate_js_in_frame(
            "CHILDFRAME", "document.getElementById('secret').textContent"
        ),
        predicate=lambda v: v == "the-cross-origin-secret-42",
    )
    print(f"  child   evaluate_js_in_frame → {secret!r}")

    # You can DRIVE it too, not just read — mutate the child's DOM:
    await tab.evaluate_js_in_frame(
        "CHILDFRAME", "document.body.style.background = 'rebeccapurple'"
    )
    bg = await tab.evaluate_js_in_frame(
        "CHILDFRAME", "getComputedStyle(document.body).backgroundColor"
    )
    print(f"  child   (after mutating it)  → background is now {bg!r}")

    assert "NULL" in from_parent or "THROW" in from_parent, "parent should be blocked"
    assert secret == "the-cross-origin-secret-42", "frame eval should read the secret"
    print("  ✓ parent is blocked; frame-scoped eval reads AND writes the child")


async def part2_live_recaptcha() -> None:
    print("\n=== Part 2: live reCAPTCHA bframe (cross-origin, real site) ===")
    # reCAPTCHA's iframes are served from google.com — which Chrome field-trial
    # isolates out-of-process. Opt in to keep them reachable. (A normal
    # BrowserPool without this flag would get FrameNotFound here.)
    browser = BrowserSession(
        BrowserConfig(extra_args=["disable-site-isolation-trials"])
    )

    async with browser:
        try:
            page = await browser.new_page("https://2captcha.com/demo/recaptcha-v2")
        except Exception as exc:
            print(f"  (skipped — could not load demo page: {exc})")
            return

        # The checkbox lives in the cross-origin `recaptcha/api2/anchor` frame,
        # which reCAPTCHA injects asynchronously after load — poll for it.
        read_checkbox = (
            "(() => { const c = document.querySelector('#recaptcha-anchor');"
            " return c ? ('checkbox role=' + c.getAttribute('role') +"
            " ' aria-checked=' + c.getAttribute('aria-checked'))"
            " : 'no checkbox yet'; })()"
        )
        inside = await _poll(
            lambda: page.evaluate_js_in_frame("recaptcha/api2/anchor", read_checkbox),
            predicate=lambda v: v and v.startswith("checkbox"),
            tries=20,
            delay=0.5,
        )

        # `frame_urls()` shows every frame the browser tracks — handy for
        # discovering the right pattern (and proof the google.com frames are
        # in-process and reachable thanks to the opt-in flag).
        print("  tracked frames:")
        for u in await page.frame_urls():
            print("    -", (u[:78] + "…") if len(u) > 78 else u)

        # From the parent, the same frame's DOM is unreachable.
        from_parent = await page.evaluate_js(
            "(() => { const f ="
            " document.querySelector('iframe[src*=\\\"api2/anchor\\\"]');"
            " return f && f.contentDocument ? 'READABLE' : 'NULL (cross-origin)'; })()"
        )
        print(f"  parent  sees anchor frame DOM → {from_parent!r}")
        if inside is None:
            print("  (reCAPTCHA anchor frame never appeared — network or layout)")
            return
        print(f"  anchor  evaluate_js_in_frame → {inside!r}")
        print("  ✓ read the reCAPTCHA checkbox from INSIDE the cross-origin frame")


async def main() -> None:
    async with BrowserPool(PoolConfig()) as pool, pool.acquire() as tab:
        await part1_self_contained(tab)
    await part2_live_recaptcha()


if __name__ == "__main__":
    asyncio.run(main())
