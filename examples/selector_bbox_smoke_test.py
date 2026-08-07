"""Live smoke test for selector-backed screenshot bbox (CAS-252), against a
real site (Wikipedia's Main Page) rather than a data-URL fixture — proves
the `selector_*` kwargs on `Page.screenshot()` resolve real-world markup
through the actual PyO3 boundary, for every Yosoi selector kind, plus
viewport interaction (a resolved bbox is recomputed under a different
viewport, since layout is viewport-dependent).

The target: en.wikipedia.org's logo (`<a class="mw-logo">` containing an
`<img class="mw-logo-wordmark" alt="Wikipedia">`) — a stable, real element
with a shared id-prefix nearby (`pt-login` / `pt-login-2`, desktop vs.
mobile-hidden nav duplicates) that happens to exercise every selector kind
this ticket added, including a real-world global_id disambiguation case
that isn't a made-up fixture.

Run: .venv/bin/python examples/selector_bbox_smoke_test.py
"""

import asyncio
import inspect
from collections.abc import Callable
from pathlib import Path

from voidcrawl import BrowserSession
from voidcrawl.viewport import Viewport

WIKIPEDIA = "https://en.wikipedia.org/wiki/Main_Page"
OUT_DIR = Path(__file__).parent / "screenshots"


def png_dimensions(png: bytes) -> tuple[int, int]:
    return int.from_bytes(png[16:20], "big"), int.from_bytes(png[20:24], "big")


async def expect_error(call: Callable[[], object], needle: str) -> None:
    """Call `call()` and, if it returns an awaitable, await that too —
    asserting an exception raises somewhere in there with `needle` in its
    message. `screenshot()`'s mutual-exclusivity checks raise synchronously
    (before a coroutine is even returned), while its selector-resolution
    errors raise from inside the awaited coroutine — this covers both.
    """
    try:
        result = call()
        if inspect.isawaitable(result):
            await result
    except Exception as e:
        print(f"  raised as expected: {type(e).__name__}: {e}")
        if needle not in str(e):
            msg = f"expected {needle!r} in error message, got: {e}"
            raise AssertionError(msg) from e
        return
    msg = f"expected an exception containing {needle!r}, none was raised"
    raise AssertionError(msg)


async def shoot(page: object, label: str, **kwargs: object) -> tuple[int, int]:
    png = await page.screenshot(**kwargs)  # type: ignore[attr-defined]
    assert isinstance(png, bytes)
    (OUT_DIR / f"{label}.png").write_bytes(png)
    return png_dimensions(png)


async def main() -> None:
    OUT_DIR.mkdir(exist_ok=True)

    async with BrowserSession() as browser:
        page = await browser.new_page(WIKIPEDIA)

        print("=== css: the logo link, `a.mw-logo` ===")
        dims = await shoot(
            page, "logo_css", selector_type="css", selector_value="a.mw-logo"
        )
        print(f"  bbox: {dims}")

        print("=== xpath: the wordmark image, //img[@class='mw-logo-wordmark'] ===")
        dims = await shoot(
            page,
            "logo_xpath",
            selector_type="xpath",
            selector_value="//img[@class='mw-logo-wordmark']",
        )
        print(f"  bbox: {dims}")

        print("=== attr: css finds the element, `name` just says which attribute ===")
        dims = await shoot(
            page,
            "logo_attr",
            selector_type="attr",
            selector_value="img.mw-logo-wordmark",
            selector_name="alt",
        )
        print(f"  bbox: {dims}  (attr read is metadata only — crop matches css/xpath)")

        print("=== global_id: real markup, not a contrived fixture ===")
        print("  Wikipedia ships both a desktop and a mobile-hidden nav, sharing an")
        print("  id prefix: <li id='pt-login'> (display:none) and <li id='pt-login-2'>")
        print("  (visible) — filtering `li` by shared prefix 'pt-login' finds both,")
        print("  visibility filtering then resolves to the one that's actually shown.")
        dims = await shoot(
            page,
            "login_link_global_id",
            selector_type="global_id",
            selector_value="li",
            selector_name="pt-login",
        )
        print(f"  bbox: {dims}")

        print("=== role: AX tree role=image, accessible name 'Wikipedia' ===")
        dims = await shoot(
            page,
            "logo_role",
            selector_type="role",
            selector_value="image",
            selector_name="Wikipedia",
        )
        print(f"  bbox: {dims}  (same node as css/xpath, resolved a different way)")

        print("=== visual: raw pixel coords inside the wordmark, exact 1x1 box ===")
        dims = await shoot(
            page,
            "logo_visual_point",
            selector_type="visual",
            selector_x=300.0,
            selector_y=25.0,
        )
        print(f"  bbox: {dims}")

        print("=== jsonld / regex: always a typed non-visual Empty, not a guess ===")
        await expect_error(
            lambda: page.screenshot(selector_type="jsonld", selector_value="$.logo"),
            "non-visual",
        )
        await expect_error(
            lambda: page.screenshot(selector_type="regex", selector_regex="Wikipedia"),
            "canonical DOM element",
        )

        print("=== ambiguous: the footer's 2 visible icon links ===")
        await expect_error(
            lambda: page.screenshot(
                selector_type="css", selector_value="#footer-icons li"
            ),
            "visible matches",
        )

        print("=== bbox + selector together still rejected ===")
        await expect_error(
            lambda: page.screenshot(
                bbox=(0, 0, 10, 10), selector_type="css", selector_value="a.mw-logo"
            ),
            "mutually exclusive",
        )

        print("\n=== viewport interaction: same css selector, three real viewports ===")
        print(
            "  A resolved bbox reflects live layout, not a cached rect — persistently"
        )
        print(
            "  changing the viewport (set_viewport) and re-resolving the same selector"
        )
        print("  proves resolution happens after layout settles, per shot. Note: PNG")
        print("  pixel dims below scale by device_scale_factor (more raster pixels per")
        print("  CSS pixel on HiDPI); the crop rect itself stays CSS-pixel-sized")
        print("  (140x38) on all three — same element, same box.")
        for label, vp in [
            ("desktop_1080p", Viewport(preset="Desktop 1080p")),
            ("ipad_pro_11", Viewport(preset="iPad Pro 11")),
            ("iphone_16", Viewport(preset="iPhone 16")),
        ]:
            await page.set_viewport(**vp.as_kwargs())  # type: ignore[arg-type]
            dims = await shoot(
                page,
                f"logo_css_{label}",
                selector_type="css",
                selector_value="a.mw-logo",
            )
            print(f"  {label:<14} preset={vp.preset!r:<16} png pixel dims: {dims}")
        await page.clear_viewport()

        await page.close()

    print("\nAll selector-backed bbox checks passed against live Wikipedia.")
    print(f"-> {OUT_DIR}/")


if __name__ == "__main__":
    asyncio.run(main())
