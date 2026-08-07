"""Integration tests for screen recording (``Page.record`` / ``start_recording``).

Requires a built extension (``./build.sh``) and Chrome/Chromium installed.

Run with:
    uv run pytest tests/test_recording.py -v
"""

from __future__ import annotations

import asyncio
import shutil
import urllib.parse
from typing import TYPE_CHECKING

import pytest

from voidcrawl import BrowserConfig, BrowserSession, Recording

if TYPE_CHECKING:
    from pathlib import Path

pytestmark = pytest.mark.skipif(
    shutil.which("chromium") is None and shutil.which("google-chrome") is None,
    reason="Chrome/Chromium not installed",
)

# A page that repaints continuously. A static page legitimately yields almost
# no frames — the screencast fires on paint, not on a clock — so anything
# asserting a frame rate has to give Chrome a reason to composite.
_ANIMATED_HTML = """<html><body style='margin:0'>
<div id='box' style='width:200px;height:120px;background:#c00'></div>
<div id='other' style='width:150px;height:90px;background:#0c0'></div>
<script>
let t = 0;
function tick() {
  t += 4;
  document.getElementById('box').style.background = 'hsl(' + (t % 360) + ',80%,50%)';
  document.getElementById('other').style.transform = 'translateX(' + (t % 50) + 'px)';
  requestAnimationFrame(tick);
}
tick();
</script></body></html>"""

ANIMATED_URL = "data:text/html," + urllib.parse.quote(_ANIMATED_HTML)


class TestRecording:
    @pytest.mark.asyncio
    async def test_records_viewport_with_real_frame_offsets(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            rec = await page.record(duration_secs=2, fps=10)

            assert isinstance(rec, Recording)
            assert len(rec.regions) == 1
            assert rec.regions[0].label == "viewport"
            assert rec.regions[0].bbox is None
            assert rec.frames_captured > 1
            assert rec.effective_fps() <= 10.5

            frames = rec.regions[0].frames
            assert len(frames) == rec.frames_captured
            assert all(len(f.data) > 0 for f in frames)
            # Offsets are measured elapsed times, not index/fps.
            offsets = [f.offset_ms for f in frames]
            assert offsets == sorted(offsets)
            assert offsets[-1] <= rec.duration_ms + 500

    @pytest.mark.asyncio
    async def test_multiple_selectors_become_separate_regions(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            rec = await page.record(
                duration_secs=2,
                selectors=[
                    {"type": "css", "value": "#box"},
                    {"type": "css", "value": "#other"},
                ],
            )

            assert [r.label for r in rec.regions] == ["0_box", "1_other"]
            assert all(r.bbox is not None for r in rec.regions)
            # One screencast feeds every region, so the frame counts match.
            assert all(len(r.frames) == rec.frames_captured for r in rec.regions)
            assert rec.regions[0].bbox != rec.regions[1].bbox
            # Different source rects must yield different pixels.
            assert rec.regions[0].frames[0].data != rec.regions[1].frames[0].data

    @pytest.mark.asyncio
    async def test_start_recording_captures_across_interaction(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            handle = await page.start_recording(duration_secs=15, fps=10)
            await page.evaluate_js(
                "document.getElementById('box').style.width = '300px'"
            )
            await asyncio.sleep(1.5)
            rec = await handle.stop()

            assert rec.frames_captured > 1
            # Stopped well before the 15s bound, so the handle -- not the
            # deadline -- ended it.
            assert rec.duration_ms < 10_000

    @pytest.mark.asyncio
    async def test_stopping_twice_raises(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            handle = await page.start_recording(duration_secs=5)
            await asyncio.sleep(0.5)
            await handle.stop()
            with pytest.raises(RuntimeError, match="already stopped"):
                await handle.stop()

    @pytest.mark.asyncio
    async def test_fps_is_a_ceiling_and_drops_are_reported(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            rec = await page.record(duration_secs=2, fps=2)

            assert rec.effective_fps() <= 3.0
            # An animating page paints far faster than 2fps, so the throttle
            # must have discarded frames rather than silently keeping them.
            assert rec.frames_dropped > 0

    @pytest.mark.asyncio
    async def test_bbox_and_selectors_are_mutually_exclusive(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            with pytest.raises(ValueError, match="mutually exclusive"):
                await page.record(
                    duration_secs=1,
                    bbox=(0, 0, 10, 10),
                    selectors=[{"type": "css", "value": "#box"}],
                )

    @pytest.mark.asyncio
    async def test_unresolvable_selector_fails_before_recording(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            loop = asyncio.get_running_loop()
            started = loop.time()
            with pytest.raises(RuntimeError, match=r"no visible target|not visible"):
                await page.record(
                    duration_secs=30,
                    selectors=[{"type": "css", "value": "#nope"}],
                )
            # Must fail up front, not after burning the full duration.
            assert loop.time() - started < 10

    @pytest.mark.asyncio
    async def test_viewport_override_does_not_leak(self) -> None:
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            before = await page.evaluate_js("window.innerWidth")
            await page.record(duration_secs=1, viewport_width=500, viewport_height=400)
            assert await page.evaluate_js("window.innerWidth") == before

    @pytest.mark.asyncio
    async def test_encoding_without_the_feature_names_the_feature(
        self, tmp_path: Path
    ) -> None:
        """A missing codec feature must fail loudly, with the frames intact --
        not silently produce nothing."""
        async with (
            BrowserSession(BrowserConfig()) as browser,
            browser.page() as page,
        ):
            await page.goto(ANIMATED_URL)
            rec = None
            failure = None
            try:
                rec = await page.record(
                    duration_secs=1, output_dir=str(tmp_path), encode=["gif"]
                )
            except RuntimeError as exc:
                failure = str(exc)

            if failure is not None:
                # Built without the feature: the error must name it, so the
                # caller knows what to enable rather than guessing.
                assert "encode-gif" in failure
            else:
                # Built with the feature on: the artifact must actually exist.
                assert rec is not None
                assert rec.regions[0].outputs
                for path in rec.regions[0].outputs:
                    assert (tmp_path / path.rsplit("/", 1)[-1]).exists()
