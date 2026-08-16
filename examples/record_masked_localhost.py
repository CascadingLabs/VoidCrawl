"""Record and mask a local HTML fixture.

Run from the repository root after ``./build.sh``::

    uv run python examples/record_masked_localhost.py

The fixture moves the password field after recording starts. The selector mask
tracks that movement; the username field and explanatory text remain visible.
Frames are written as PNG so the black pixels can be inspected exactly.
"""

from __future__ import annotations

import asyncio
import functools
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from voidcrawl import BrowserConfig, BrowserSession

ROOT = Path(__file__).resolve().parent / "fixtures"
OUTPUT = Path("output/masked-localhost")


def serve_fixture() -> tuple[ThreadingHTTPServer, str]:
    handler = functools.partial(SimpleHTTPRequestHandler, directory=str(ROOT))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/obfuscation_poc.html"


async def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    server, url = serve_fixture()
    try:
        async with BrowserSession(BrowserConfig(headless=True)) as browser:
            page = await browser.new_page_in_window(url)
            await page.wait_for_network_idle(timeout=5.0)

            handle = await page.start_recording(
                duration_secs=4,
                fps=10,
                viewport_width=700,
                viewport_height=500,
                output_dir=str(OUTPUT),
                frame_format="png",
                write_frames=True,
                selectors=[{"type": "css", "value": "#login-form"}],
                masks=[{"type": "css", "value": "#password", "label": "password"}],
                mask_pad=2,
            )
            await asyncio.sleep(2.5)
            recording = await handle.stop()

        region = recording.regions[0]
        mask = recording.masks[0]
        print(f"URL: {url}")
        print(
            f"frames: {recording.frames_captured}, "
            f"effective fps: {recording.effective_fps():.1f}"
        )
        print(f"region: {region.label}, bbox: {region.bbox}")
        print(
            f"mask: {mask.label}, bbox: {mask.bbox}, tracked: {mask.tracked}, "
            f"unresolved ticks: {mask.unresolved_ticks}, "
            f"stale frames: {mask.stale_frames}"
        )
        print(f"frames: {OUTPUT / region.label}")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    asyncio.run(main())
