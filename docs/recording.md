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

### OpenSesame/noVNC interaction evidence

For an authorized human or OpenSesame login in a headful browser, use
`examples/opensesame_recorded_novnc_login.py`. It starts one tab, records the
same tab while the operator works through noVNC, masks password selectors, and
writes an MP4 plus an append-only `audit.jsonl` containing attach coordinates,
response provenance, mask reports, frame counts, and output paths. Credentials,
HTML, cookies, and actor command arguments are deliberately excluded.

```bash
./docker/run-headful.sh -d
ffmpeg -version
export AUTHORIZED_LOGIN_URL='https://your-authorized-login.example/path'
uv run python examples/opensesame_recorded_novnc_login.py \
  --docker-headful \
  --url "$AUTHORIZED_LOGIN_URL"
```

Replace the example URL with a real authorized target; `example.test` is a
non-routable documentation placeholder. Pass `--opensesame-command "python
/path/to/actor.py"` to run an external
actor. It receives
`VOIDCRAWL_OPENSESAME_ATTACH_JSON`, containing the same-tab
`websocket_url`/`target_id` and the noVNC/VNC links. Without that option, the
example waits for a human to complete the flow in noVNC.

## Masking

`masks` paints rectangles solid black in every frame, **before** anything is
cropped, written, or encoded. It exists because there is no other seam: between
Chrome compositing a frame and that frame hitting disk, nothing downstream can
intervene, so whatever is on screen is already in the artifact by the time
anyone else gets a say.

```python
rec = await page.start_recording(
    masks=[{"type": "css", "value": "#password"}],
    dir="./out",
    encode=["mp4"],
)
```

### What this is and is not

VoidCrawl supplies the mechanism, not the judgment. A mask is a rectangle.
There are no classifiers, no presets, no `input[type=password]` special-casing,
and nothing here decides what counts as sensitive or whether a finished
recording is safe to share. You name the regions; the obfuscation harness that
decides which regions to name lives above this library.

So masking a recording is **not** a claim that the recording is clean — only
that the named rectangles were covered, which the `masks` report says exactly.
`examples/record_masked_login.py` demonstrates the boundary by accident: it
masks the password *field*, and the page's own instruction text — which prints
the same password — stays perfectly legible, because nobody named it.

### Fixed rectangles and selectors

A mask is either a literal rectangle or a selector to resolve into one:

```python
masks=[
    {"bbox": (100, 240, 320, 32)},                         # no DOM work at all
    {"selector": {"type": "css", "value": "#token"}},      # resolved for you
    {"selector": {...}, "track": False, "label": "token"}, # pinned, named
]
```

A bare selector dict is accepted as shorthand for the second form. Selector
masks resolve through the same path as crop selectors, so all 8 Yosoi selector
kinds work — and a mask that matches nothing, is ambiguous, or is non-visual
fails the call before any frame is captured. An unresolvable mask is a hole in
an artifact you believe is covered, so it is never silently skipped.

### Masks track; crops do not

Crop regions are resolved once and held fixed. A crop that drifts is cosmetic;
a mask that drifts uncovers the thing it was asked to cover. So selector masks
are re-resolved on a timer (at most 5 Hz) and each frame is masked with the
rectangles current when it was captured, under two conservative rules:

- a frame is masked with the **union** of the current and previous tick's
  rectangles, so an element caught mid-scroll is covered in both places;
- a mask whose selector stops resolving **keeps its last known rectangle**
  rather than uncovering.

Neither rule is a risk assessment — both are just "cover more, not less". What
happened is reported per mask, and what it means is yours to decide:

| Field | Meaning |
| --- | --- |
| `label` | Name, from `label` or derived from the selector. |
| `bbox` | The rectangle as first resolved. |
| `tracked` | Whether it was re-resolved during the recording. |
| `unresolved_ticks` | Ticks where re-resolution failed (element gone, hidden, ambiguous). |
| `stale_frames` | Frames captured while the most recent re-resolution had failed. |

A navigation mid-recording is the common source of `unresolved_ticks`: the old
document's element is gone, so the mask holds its last rectangle over a page it
no longer belongs to. Non-zero `stale_frames` is a signal to look at the
artifact before sharing it.

### Cost and caveats

- Masking costs one decode + re-encode per frame — once per frame, not once per
  frame per region. An unmasked recording pays nothing.
- The pixels are covered in-process, after Chrome composited them and after the
  frame was decoded. They never reach disk, an encoder, or a caller — but they
  do exist in memory first.
- JPEG is lossy, so a black fill re-encoded at quality 80 is *near*-black rather
  than exactly `#000`. No original pixel survives either way; use
  `frame_format="png"` when you want to assert on exact values.
- `mask_pad` (default 2 CSS px) grows each rectangle outward to swallow
  antialiasing at the edges. Set `0` for the exact rectangle.
- Masks are orthogonal to crops. Cropping to a form and masking a field inside
  it is the normal case, and the crop is always cut from an already-masked
  frame.

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

`./build.sh` enables MP4/WebM encoding for the Python extension. It requires
an `ffmpeg` binary on `PATH` at recording time:

```bash
ffmpeg -version
./build.sh
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
| `masks` | One per requested mask; empty means nothing was asked to be covered. |
| `frames_captured` | Frames kept, per region. |
| `frames_dropped` | Frames discarded by the `fps` ceiling or frame cap. |
| `effective_fps()` | Rate actually achieved. Report this, not the requested `fps`. |
| `duration_ms` | Wall-clock span from screencast start to stop. |
| `foregrounded` | Whether the capture lock was held (see the window rule). |
| `device_pixel_ratio` | For translating frame pixels back to CSS pixels. |

Frames expose `index`, `offset_ms`, and `data`. Regions expose `label`,
`bbox`, `frames`, and `outputs`.
