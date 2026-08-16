"""Record a real login and black out the password field in every frame.

    ./build.sh
    uv run --with pillow python examples/record_masked_login.py

Writes ``output/masked-login/viewport.mp4`` and then *proves* the redaction by
reading frames back off disk: the masked rectangle must be black — including
after the page scrolls and the field has moved, which is what mask tracking
buys over a fixed rectangle — while the username field beside it must not be.

VoidCrawl covers the rectangles it is told to cover and reports what it
covered. Deciding that a password field is what deserves covering is this
script's job, not the library's — and the artifact shows exactly why that
boundary matters: this page prints the demo password in its own instruction
text, which is still perfectly legible in the recording, because nobody named
that rectangle. Masking is opt-in redaction, not a classifier.
"""

from __future__ import annotations

import asyncio
from pathlib import Path

from PIL import Image

from voidcrawl import BrowserConfig, BrowserSession

URL = "https://the-internet.herokuapp.com/login"
OUTPUT_DIR = Path("output/masked-login")
RECT_JS = (
    "(() => { const r = document.querySelector({sel!r}).getBoundingClientRect();"
    " return [Math.round(r.x), Math.round(r.y), "
    "Math.round(r.width), Math.round(r.height)]; })()"
)


def rect_js(selector: str) -> str:
    return RECT_JS.replace("{sel!r}", f"'{selector}'")


def black_fraction(image: Image.Image) -> float:
    rgb = image.tobytes()
    black = sum(
        1 for i in range(0, len(rgb), 3) if rgb[i] == rgb[i + 1] == rgb[i + 2] == 0
    )
    return black / (len(rgb) / 3)


async def main() -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    async with BrowserSession(BrowserConfig(headless=True)) as browser:
        page = await browser.new_page_in_window(URL)
        await page.wait_for_network_idle(timeout=15.0)

        handle = await page.start_recording(
            duration_secs=15,
            fps=10,
            output_dir=str(OUTPUT_DIR),
            frame_format="png",
            write_frames=True,
            # A short viewport, so the page has somewhere to scroll to.
            viewport_width=1000,
            viewport_height=400,
            masks=[{"type": "css", "value": "#password"}],
            encode=["mp4"],
        )
        try:
            await page.type_into("#username", "tomsmith")
            await page.type_into("#password", "SuperSecretPassword!")
            await asyncio.sleep(1.5)

            # Move the masked element. A fixed rectangle would uncover it here.
            await page.evaluate_js("window.scrollBy(0, 120)")
            await asyncio.sleep(1.5)
            moved = await page.evaluate_js(rect_js("#password"))
            username = await page.evaluate_js(rect_js("#username"))
        finally:
            recording = await handle.stop()

    mask = recording.masks[0]
    print(f"{recording.frames_captured} frames in {recording.duration_ms:.0f} ms")
    print(
        f"mask {mask.label}: bbox={mask.bbox} tracked={mask.tracked} "
        f"unresolved_ticks={mask.unresolved_ticks} stale_frames={mask.stale_frames}"
    )
    print(f"password field moved to {moved}")
    print(f"video: {recording.regions[0].outputs[0]}")

    # The proof: read frames back and look at the pixels that shipped.
    frames = sorted((OUTPUT_DIR / recording.regions[0].label).glob("*.png"))
    first = Image.open(frames[0]).convert("RGB")
    last = Image.open(frames[-1]).convert("RGB")

    x, y, w, h = mask.bbox
    assert first.getpixel((x + w // 2, y + h // 2)) == (0, 0, 0), (
        "password field was not covered"
    )

    # The username field, one row up, must survive — a mask that blacked out
    # the whole frame would also pass the assertion above.
    ux, uy, uw, uh = username
    control = last.getpixel((ux + uw // 2, uy + uh // 2))
    assert control != (0, 0, 0), f"masking covered more than it was asked to: {control}"
    covered = black_fraction(last)
    assert covered < 0.2, f"{covered:.0%} of the frame is black — that is not a mask"

    mx, my, mw, mh = moved
    after = last.getpixel((mx + mw // 2, my + mh // 2))
    assert after == (0, 0, 0), f"the mask did not follow the scroll: {after}"

    print(
        f"OK — covered at {mask.bbox} in {frames[0].name} "
        f"and at {moved} in {frames[-1].name}; "
        f"username px={control}, {covered:.1%} of the frame black"
    )


if __name__ == "__main__":
    asyncio.run(main())
