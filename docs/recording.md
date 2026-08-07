# Screen recording

Capture a page as a sequence of timestamped frames — the moving-picture
counterpart to [screenshots](api-reference.md). The options deliberately
mirror `screenshot`'s (`viewport`, `scroll`, `bbox`, Yosoi selector crops), so
a caller who can screenshot a region can record it by changing one call.

```python
rec = await page.record(duration_secs=5, fps=10)
print(rec.frames_captured, "frames at %.1f fps" % rec.effective_fps())
```

## How it differs from a screenshot

Frames come from CDP's `Page.startScreencast`. Three consequences follow, and
they are the whole design:

**1. Viewport only.** A screencast frame is what is composited on screen.
There is no `full_page` equivalent — CDP does not offer one. Use `viewport` to
make the visible area bigger, or `scroll` to choose which part of a long page
is on screen when recording starts. For the same reason `bbox` here is
**viewport-relative**, unlike `screenshot`'s page-relative one.

**2. Frames arrive on paint, not on a clock.** Chrome emits a frame when it
swaps one, so a static page yields almost nothing and a busy page yields
bursts. `fps` is a *ceiling* applied client-side, never a guarantee — check
`effective_fps()` rather than assuming the requested rate. Every frame carries
its real `offset_ms` from recording start, so an encoder can resample honestly
instead of pretending the rate was uniform. Frames discarded by the ceiling
are reported in `frames_dropped`.

**3. Selectors are plural.** A screenshot crops to at most one rectangle; a
recording can carry several. Each entry in `selectors` becomes its own region
with its own cropped frame sequence, all cut from the *same* screencast —
recording three components of a page costs one screencast, not three.

```python
rec = await page.record(
    duration_secs=5,
    selectors=[
        {"type": "css", "value": "#chart"},
        {"type": "role", "value": "button", "name": "Play"},
    ],
)
for region in rec.regions:
    print(region.label, region.bbox, len(region.frames))
```

Each selector is resolved to a rectangle **once, when recording starts**, and
that rectangle is then fixed. An element that moves or resizes mid-recording
drifts out of its crop. That matches `screenshot(selector=...)` semantics and
keeps the per-frame cost at zero DOM round-trips; per-frame tracking would cap
the frame rate on a DOM query.

A selector that matches nothing, is ambiguous, or is inherently non-visual
(`jsonld`/`regex`) fails the call *before* any frame is captured, rather than
burning the full duration and returning empty regions.

## Recording an interaction

`record()` is one-shot. To record while you drive the page, start and stop it
explicitly:

```python
handle = await page.start_recording(duration_secs=30)
await page.click("#play")
await page.type_text("#search", "query")
rec = await handle.stop()
```

`duration_secs` becomes a hard upper bound: the recording stops itself at that
point even if `stop()` is never called, so an abandoned recording cannot hold
the browser open.

## Concurrency: the window rule

This is the one piece of Chrome behavior worth understanding before recording
at scale.

**Chrome composites only the frontmost tab of a window.** Tabs opened by
`new_page` — and every tab in the pool — share one window, so the moment a
sibling tab captures (which foregrounds it), the recorded tab stops painting
and the screencast goes quiet. Measured on a continuously animating page over
3 seconds:

| tab placement | foregrounded | frames captured |
| --- | --- | --- |
| shared window | no | 1 |
| shared window | yes | 27 |
| **own window** | **no** | **28** |

The `--disable-backgrounding-occluded-windows` and
`--disable-renderer-backgrounding` launch flags do *not* rescue the
shared-window case; occlusion *within* a window is a different mechanism.

So a recording on a shared-window tab must pin itself to the foreground and
hold the browser-wide capture lock for its duration — which blocks screenshots
and recordings on every sibling tab of that browser. A tab **alone in its
window** cannot be occluded that way, records at full rate, and needs neither.

You do not have to manage this: `foreground` defaults to auto-detection via
`alone_in_window()`, and picks the correct behavior either way. To get the
concurrent path, put the recorded page in its own window:

```python
page = await session.new_page_in_window(url)   # its own browser window
assert await page.alone_in_window()
rec = await page.record(duration_secs=10)      # runs concurrently
assert not rec.foregrounded
```

One sharp edge: a plain `new_page` opens its tab in the **most recently
active** window, so creating a page *after* `new_page_in_window` can drop it
into that window and re-introduce the contention. Create the recording window
last, or check `alone_in_window()`. Every `Recording` reports `foregrounded`
so you can tell which path you got.

## Output

A recording always carries its frames in memory. Encoding to a single playable
file is opt-in and lives behind cargo features, because the frame sequence is
the substrate and the container is a policy choice:

| encoding | feature | notes |
| --- | --- | --- |
| `gif` | `encode-gif` | Pure Rust, no external binary. Large files, 256 colors. |
| `mp4` | `encode-ffmpeg` | H.264 via an `ffmpeg` binary on PATH. |
| `webm` | `encode-ffmpeg` | VP9 via an `ffmpeg` binary on PATH. |

```bash
cargo build --features encode-gif,encode-ffmpeg
```

```python
rec = await page.record(duration_secs=5, dir="./out", encode=["gif"])
print(rec.regions[0].outputs)   # ['./out/viewport.gif']
```

Requesting an encoding whose feature is off raises an error naming the missing
feature rather than silently producing nothing — and the frames survive either
way. Both encoders drive timing off each frame's real offset rather than
assuming a constant rate, so playback matches what the page actually did.

Pass `write_frames=True` to also write the individual frames to
`dir/<region>/00000.jpg`.

## MCP tools

- **`record`** — load a URL and record it for a fixed duration.
- **`session_record_start`** / **`session_record_stop`** — record an open
  session while you drive it. One recording per session at a time; a second
  start is refused rather than silently discarding the first.

These do **not** return frames inline — a recording is hundreds of images, and
streaming them would swamp an agent's context. Frames and artifacts are
written to disk and the response carries the output directory, per-region
paths, and counts. Read one frame from disk if you need to see it.

Recording length is capped at 120s, and `session_record_start` defaults to a
30s bound.

## Reference

| Field | Meaning |
| --- | --- |
| `regions` | One per requested region; a single `"viewport"` region by default. |
| `frames_captured` | Frames kept, per region. |
| `frames_dropped` | Frames discarded by the `fps` ceiling or frame cap. |
| `effective_fps()` | Rate actually achieved. Report this, not the requested `fps`. |
| `duration_ms` | Wall-clock span from screencast start to stop. |
| `foregrounded` | Whether the capture lock was held (see the window rule). |
| `device_pixel_ratio` | For translating frame pixels back to CSS pixels. |

Frames expose `index`, `offset_ms`, and `data`. Regions expose `label`,
`bbox`, `frames`, and `outputs`.
