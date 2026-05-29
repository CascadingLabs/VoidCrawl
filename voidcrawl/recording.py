"""Record a browser session to video — native CDP screencast + ffmpeg assembly.

This is the ergonomic front door to :meth:`voidcrawl.Page.start_screencast`.
Use it as an async context manager around any flow you want to capture::

    from voidcrawl import BrowserSession, BrowserConfig, record

    async with BrowserSession(BrowserConfig()) as browser:
        page = await browser.new_page("https://example.com")
        async with record(page, "demo.mp4", fps=12, also_gif=True) as rec:
            await page.goto("https://example.com")
            # ... drive the page, inject an on-page HUD, click, etc. ...
        print(rec.mp4_path, rec.gif_path, rec.frame_count)

CDP pushes a frame only when the page actually changes, and every frame is the
same viewport size — so unlike the old screenshot-per-step → ffmpeg hand-roll,
there is no per-frame canvas normalization to do, and playback timing follows
the *real* session clock via the frame timestamps. ``ffmpeg`` must be on PATH.
"""

from __future__ import annotations

import asyncio
import itertools
import shutil
import statistics
import subprocess
import tempfile
from contextlib import asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from voidcrawl._ext import Screencast, ScreencastFrame

# Smallest per-frame hold; guards against zero/sub-ms timestamp deltas blowing
# the effective frame rate up to absurd values.
_MIN_FRAME_SECONDS = 1.0 / 60.0


class RecordingError(RuntimeError):
    """Raised when a recording cannot be assembled.

    Causes: no frames were captured, ``ffmpeg`` is not installed, or ffmpeg
    exited non-zero.
    """


class _Recordable(Protocol):
    """Anything that can start a screencast — :class:`Page` or :class:`PooledTab`."""

    async def start_screencast(
        self,
        format: str | None = ...,  # noqa: A002 — mirrors the native CDP kwarg
        quality: int | None = ...,
        max_width: int | None = ...,
        max_height: int | None = ...,
        every_nth_frame: int | None = ...,
    ) -> Screencast: ...


class Recording:
    """Handle for an in-progress / completed recording.

    Yielded by :func:`record`. While inside the ``async with`` block it is just
    a marker; after the block exits the frames have been captured and assembled,
    and these attributes are populated:

    Attributes:
        mp4_path: Path to the written mp4, or ``None`` if the target was a gif.
        gif_path: Path to the written gif, or ``None``.
        frame_count: Number of frames captured.
    """

    def __init__(self, path: Path, fps: int, also_gif: bool) -> None:
        self._path = path
        self._fps = fps
        self._also_gif = also_gif
        self._frames: list[ScreencastFrame] = []
        self.mp4_path: Path | None = None
        self.gif_path: Path | None = None
        self.frame_count: int = 0

    @property
    def frames(self) -> list[ScreencastFrame]:
        """The raw captured frames (available after the context exits)."""
        return self._frames

    def _frame_durations(self) -> list[float]:
        """Per-frame display seconds, from real timestamps when Chrome gave them.

        Falls back to a uniform ``1/fps`` cadence when timestamps are missing.
        The last frame is held for the median of the others so the final state
        lingers instead of flashing past.
        """
        stamps = [f.timestamp for f in self._frames]
        valid = [s for s in stamps if s is not None]
        if len(self._frames) > 1 and len(valid) == len(self._frames):
            gaps = [
                max(b - a, _MIN_FRAME_SECONDS) for a, b in itertools.pairwise(valid)
            ]
            gaps.append(max(statistics.median(gaps), 1.0 / self._fps))
            return gaps
        return [1.0 / self._fps] * len(self._frames)

    def _assemble(self) -> None:
        """Write frames to a temp dir and stitch them with ffmpeg.

        Runs synchronously; :func:`record` offloads it to a worker thread.
        """
        if not self._frames:
            raise RecordingError(
                "no frames captured — was the page changing during the recording?"
            )
        if shutil.which("ffmpeg") is None:
            raise RecordingError(
                "ffmpeg not found on PATH; install it to assemble recordings"
            )

        ext = ".png" if self._frames[0].data[:8] == b"\x89PNG\r\n\x1a\n" else ".jpg"
        durations = self._frame_durations()
        self.frame_count = len(self._frames)

        with tempfile.TemporaryDirectory(prefix="voidcrawl-rec-") as tmp:
            tmpdir = Path(tmp)
            names: list[str] = []
            for i, frame in enumerate(self._frames):
                name = f"frame_{i:05d}{ext}"
                (tmpdir / name).write_bytes(frame.data)
                names.append(name)

            # ffmpeg concat demuxer: one `file`+`duration` pair per frame, with
            # the final file repeated (concat honors a duration only when another
            # entry follows it). This preserves the real per-frame timing.
            lines: list[str] = []
            for name, dur in zip(names, durations, strict=True):
                lines.append(f"file '{name}'")
                lines.append(f"duration {dur:.3f}")
            lines.append(f"file '{names[-1]}'")
            concat = tmpdir / "frames.txt"
            concat.write_text("\n".join(lines) + "\n")

            target_is_gif = self._path.suffix.lower() == ".gif"
            mp4_path = (
                self._path if not target_is_gif else self._path.with_suffix(".mp4")
            )
            self._path.parent.mkdir(parents=True, exist_ok=True)

            # Pad odd dimensions to even — libx264 + yuv420p requires it.
            _run_ffmpeg(
                [
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    str(concat),
                    "-vsync",
                    "vfr",
                    "-vf",
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    str(mp4_path),
                ]
            )
            if not target_is_gif:
                self.mp4_path = mp4_path

            if target_is_gif or self._also_gif:
                gif_path = (
                    self._path if target_is_gif else self._path.with_suffix(".gif")
                )
                # Derive the gif from the uniform mp4 (high-quality palette pass).
                _run_ffmpeg(
                    [
                        "-i",
                        str(mp4_path),
                        "-vf",
                        f"fps={self._fps},scale=iw:-1:flags=lanczos,"
                        "split[a][b];[a]palettegen[p];[b][p]paletteuse",
                        str(gif_path),
                    ]
                )
                self.gif_path = gif_path
                if target_is_gif:
                    # Caller asked only for a gif; drop the scratch mp4.
                    mp4_path.unlink(missing_ok=True)


def _run_ffmpeg(args: list[str]) -> None:
    """Invoke ffmpeg quietly, raising :class:`RecordingError` on failure."""
    proc = subprocess.run(
        ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", *args],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise RecordingError(
            f"ffmpeg failed ({proc.returncode}): {proc.stderr.strip()}"
        )


@asynccontextmanager
async def record(
    page: _Recordable,
    path: str | Path,
    *,
    fps: int = 12,
    format: str = "jpeg",  # noqa: A002 — mirrors the native CDP kwarg
    quality: int = 80,
    max_width: int | None = None,
    max_height: int | None = None,
    also_gif: bool = False,
) -> AsyncIterator[Recording]:
    """Record everything that happens on *page* inside the block to *path*.

    Starts a CDP screencast on entry; on exit, stops it, collects the frames,
    and stitches them into video with ffmpeg. The page stays fully usable while
    recording — drive the flow and inject an on-page HUD between steps.

    Args:
        page: A :class:`~voidcrawl.Page` or :class:`~voidcrawl.PooledTab`.
        path: Output file. ``.mp4`` writes video; ``.gif`` writes a gif (via a
            scratch mp4). With ``also_gif=True`` an ``.mp4`` target also yields a
            sibling ``.gif``.
        fps: Target frame rate for gif output and the fixed-cadence fallback.
        format: Screencast frame format, ``"jpeg"`` (default) or ``"png"``.
        quality: JPEG quality 0..100 (ignored for PNG).
        max_width: Cap frame width in device pixels (Chrome scales to fit).
        max_height: Cap frame height in device pixels.
        also_gif: When the target is mp4, also write a sibling gif.

    Yields:
        A :class:`Recording`; after the block its ``mp4_path`` / ``gif_path`` /
        ``frame_count`` are populated.

    Raises:
        RecordingError: No frames captured, ffmpeg missing, or ffmpeg failed.
    """
    cast = await page.start_screencast(
        format=format,
        quality=quality,
        max_width=max_width,
        max_height=max_height,
    )
    rec = Recording(Path(path), fps=fps, also_gif=also_gif)
    try:
        yield rec
    finally:
        rec._frames = await cast.stop()
        await asyncio.to_thread(rec._assemble)
